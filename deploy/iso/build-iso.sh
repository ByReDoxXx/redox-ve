#!/bin/sh
# RedoX VE kurulum ISO'su üretir.
#
# Debian'ın ağdan kurulum (netinst) kalıbını alır; içine yanıt dosyasını, RedoX VE ikili dosyasını ve kurulum
# betiğini koyar. Ortaya çıkan ISO bir USB belleğe yazılıp önyüklenir: dil, makine adı, kök parolası ve disk
# sorulur, gerisi kendiliğinden kurulur ve sunucu açıldığında konsolda panelin adresi yazar.
#
#   sh deploy/iso/build-iso.sh --debian-iso debian-13.x.y-amd64-netinst.iso --binary YOL/redox [--out redox-ve.iso]
#       [--answers DOSYA]   # ek yanıtlar: kurucunun sorduklarını da önceden yanıtlar (hiç soru sormayan kurulum)
#
# Gerekenler: xorriso, cpio, gzip (Debian/Ubuntu: sudo apt install xorriso cpio).
# İkili dosya durağan bağlanmış olmalıdır (musl), yoksa derlendiği sistemden eski bir Debian'da çalışmaz:
#   rustup target add x86_64-unknown-linux-musl && sudo apt install musl-tools
#   cargo build --release --target x86_64-unknown-linux-musl
# Kurulum sırasında hedef makinenin internete çıkması gerekir (paketler Debian deposundan iner).
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
DEPLOY=$(dirname "$HERE")
ISO=""
BINARY=""
OUT="redox-ve.iso"
ALLOW_DYNAMIC=0
ANSWERS=""

die() { echo "hata / error: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --debian-iso) ISO="$2"; shift 2 ;;
        --binary) BINARY="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --answers) ANSWERS="$2"; shift 2 ;;
        --allow-dynamic) ALLOW_DYNAMIC=1; shift ;;
        -h|--help) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "bilinmeyen seçenek / unknown option: $1" ;;
    esac
done

[ -n "$ISO" ] && [ -f "$ISO" ] || die "--debian-iso DOSYA gerekli (Debian netinst kalıbı) / is required"
[ -n "$BINARY" ] && [ -f "$BINARY" ] || die "--binary YOL gerekli (redox ikili dosyası) / is required"
for tool in xorriso cpio gzip; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool kurulu değil / is not installed"
done
if [ "$ALLOW_DYNAMIC" = 0 ] && command -v file >/dev/null 2>&1 && ! file "$BINARY" | grep -Eq 'statically linked|static-pie linked'; then
    die "ikili dosya durağan bağlanmamış; musl hedefiyle derleyin ya da --allow-dynamic verin / the binary is not statically linked; build for the musl target or pass --allow-dynamic"
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

[ -z "$ANSWERS" ] || [ -f "$ANSWERS" ] || die "$ANSWERS yok / not found"
mkdir -p "$WORK/pre"
cat "$HERE/preseed.cfg" > "$WORK/pre/preseed.cfg"
[ -z "$ANSWERS" ] || cat "$ANSWERS" >> "$WORK/pre/preseed.cfg"

extract() { xorriso -osirrox on -indev "$ISO" -extract "$1" "$2" >/dev/null 2>&1; }

# Yanıt dosyası kurucunun başlangıç dosya sistemine (initrd) eklenir: /preseed.cfg kendiliğinden okunur.
add_preseed() {
    # $1: ISO içindeki initrd yolu, $2: çalışma dizinindeki ad
    extract "$1" "$WORK/$2.gz" || return 1
    chmod u+w "$WORK/$2.gz"
    gzip -d "$WORK/$2.gz"
    (cd "$WORK/pre" && echo preseed.cfg | cpio -H newc -o -A -F "$WORK/$2" 2>/dev/null)
    gzip -9 "$WORK/$2"
}
add_preseed /install.amd/initrd.gz initrd || die "kalıpta /install.amd/initrd.gz yok; bu bir Debian amd64 netinst kalıbı mı? / not a Debian amd64 netinst image?"
MAPS="-map $WORK/initrd.gz /install.amd/initrd.gz"
if add_preseed /install.amd/gtk/initrd.gz initrd-gtk; then
    MAPS="$MAPS -map $WORK/initrd-gtk.gz /install.amd/gtk/initrd.gz"
fi

# Kurulum ortamına konacak dosyalar.
mkdir -p "$WORK/redox"
install -m 0755 "$BINARY" "$WORK/redox/redox"
install -m 0755 "$DEPLOY/install.sh" "$WORK/redox/install.sh"
install -m 0644 "$DEPLOY/redox-host.service" "$WORK/redox/redox-host.service"
install -m 0644 "$DEPLOY/redox-net.service" "$DEPLOY/redox-net.path" "$DEPLOY/redox-ksm.service" "$DEPLOY/redox-hostnet.service" "$DEPLOY/redox-hostnet.path" "$DEPLOY/redox-dnsmasq@.service" "$DEPLOY/redox-health.service" "$DEPLOY/redox-health.timer" "$DEPLOY/redox-update.service" "$DEPLOY/redox-update.path" "$WORK/redox/"
MAPS="$MAPS -map $WORK/redox /redox"

# Önyükleme menüleri: ilk seçenek RedoX VE kurulumu olur (BIOS: isolinux, UEFI: grub).
if extract /isolinux/txt.cfg "$WORK/txt.cfg"; then
    chmod u+w "$WORK/txt.cfg"
    cat > "$WORK/txt.cfg" <<'EOF'
default redox
label redox
	menu label ^RedoX VE kur / Install RedoX VE
	menu default
	kernel /install.amd/vmlinuz
	append vga=788 initrd=/install.amd/initrd.gz --- quiet
EOF
    MAPS="$MAPS -map $WORK/txt.cfg /isolinux/txt.cfg"
fi
# Debian'ın BIOS menüsü 30 saniye tuşa basılmazsa sesli (erişilebilir) kurulumu kendiliğinden başlatır; bu
# sayaç kaldırılır, menü seçim yapılana dek bekler. Sesli kurulum seçeneği menüde durur.
if extract /isolinux/spkgtk.cfg "$WORK/spkgtk.cfg"; then
    chmod u+w "$WORK/spkgtk.cfg"
    grep -Ev '^(timeout|ontimeout|menu autoboot)' "$WORK/spkgtk.cfg" > "$WORK/spkgtk.new" && mv "$WORK/spkgtk.new" "$WORK/spkgtk.cfg"
    MAPS="$MAPS -map $WORK/spkgtk.cfg /isolinux/spkgtk.cfg"
fi
# Taban olarak daha önce üretilmiş bir RedoX VE kalıbı da verilebilir (yeni sürümü Debian kalıbını yeniden
# indirmeden koymak için): menü girdisi zaten varsa ikinci kez eklenmez.
if extract /boot/grub/grub.cfg "$WORK/grub.cfg" && ! grep -q 'RedoX VE kur' "$WORK/grub.cfg"; then
    chmod u+w "$WORK/grub.cfg"
    # Var olan menünün başına RedoX girdisi eklenir; yazı tipi ve tema satırları korunur.
    awk '
        !done && /^menuentry / {
            print "menuentry \"RedoX VE kur / Install RedoX VE\" {"
            print "    set background_color=black"
            print "    linux    /install.amd/vmlinuz vga=788 --- quiet"
            print "    initrd   /install.amd/initrd.gz"
            print "}"
            done = 1
        }
        { print }
    ' "$WORK/grub.cfg" > "$WORK/grub.new" && mv "$WORK/grub.new" "$WORK/grub.cfg"
    grep -q '^set default=' "$WORK/grub.cfg" || sed -i '1i set default=0' "$WORK/grub.cfg"
    MAPS="$MAPS -map $WORK/grub.cfg /boot/grub/grub.cfg"
fi

# Önyükleme kayıtları özgün kalıptan aynen yeniden kurulur (BIOS ve UEFI, USB bellekten de açılır).
rm -f "$OUT"
# shellcheck disable=SC2086
xorriso -indev "$ISO" -outdev "$OUT" -boot_image any replay $MAPS -commit >/dev/null 2>"$WORK/xorriso.log" \
    || { cat "$WORK/xorriso.log" >&2; die "ISO yazılamadı / could not write the ISO"; }

echo "Tamam / Done: $OUT ($(du -h "$OUT" | cut -f1))"
echo "USB belleğe yazmak için / To write it to a USB stick:  sudo dd if=$OUT of=/dev/sdX bs=4M status=progress conv=fsync"
