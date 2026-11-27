#!/bin/sh
# RedoX VE kurulum betiği.
#
# Yeni kurulmuş bir Debian 12+/Ubuntu 22.04+ sunucuyu RedoX VE sunucusuna çevirir: paketleri kurar, hizmet
# kullanıcısını ve systemd birimini hazırlar, https://<adres>:8006 üzerinde (kendinden imzalı sertifikayla)
# yayına alır ve konsol açılış ekranına panelin adresini yazar. Kök olarak çalıştırılır:
#
#   sudo sh deploy/install.sh --binary target/release/redox [--bridge eth0]
#
# Kurulum ISO'su aynı betiği --from-installer ile çağırır (deploy/iso/).
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
BINARY=""
BRIDGE_PORT=""
FROM_INSTALLER=0
DRY_RUN=0
TLS=1
ROOT="${REDOX_INSTALL_ROOT:-}"   # sınama için: dosyalar bu dizinin altına yazılır
LANGUAGE="${REDOX_LANG:-tr}"

say() { if [ "$LANGUAGE" = en ]; then echo "==> $2"; else echo "==> $1"; fi; }
die() { if [ "$LANGUAGE" = en ]; then echo "error: $2" >&2; else echo "hata: $1" >&2; fi; exit 1; }
run() { if [ "$DRY_RUN" = 1 ]; then echo "+ $*"; else "$@"; fi; }

while [ $# -gt 0 ]; do
    case "$1" in
        --binary) BINARY="$2"; shift 2 ;;
        --bridge) BRIDGE_PORT="$2"; shift 2 ;;
        --lang) LANGUAGE="$2"; shift 2 ;;
        --no-tls) TLS=0; shift ;;
        --from-installer) FROM_INSTALLER=1; shift ;;
        --dry-run) DRY_RUN=1; shift ;;
        -h|--help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "bilinmeyen seçenek: $1" "unknown option: $1" ;;
    esac
done

[ "$DRY_RUN" = 1 ] || [ -n "$ROOT" ] || [ "$(id -u)" = 0 ] || die "kök olarak çalıştırın (sudo)" "run as root (sudo)"
if [ -z "$BINARY" ]; then
    for candidate in "$HERE/redox" "$HERE/../target/release/redox" "$HERE/../target/x86_64-unknown-linux-musl/release/redox"; do
        if [ -f "$candidate" ]; then BINARY="$candidate"; break; fi
    done
fi
[ -n "$BINARY" ] && [ -f "$BINARY" ] || die "redox ikili dosyası bulunamadı; --binary YOL verin" "redox binary not found; pass --binary PATH"
for unit in redox-host.service redox-net.service redox-net.path; do
    [ -f "$HERE/$unit" ] || die "$HERE/$unit yok" "$HERE/$unit is missing"
done

# ---------------------------------------------------------------- paketler
PACKAGES="qemu-system-x86 qemu-utils ovmf swtpm swtpm-tools lxc lxc-templates uidmap acl rsync nftables iproute2 bridge-utils ca-certificates"
[ "$TLS" = 1 ] && PACKAGES="$PACKAGES caddy"
say "Paketler kuruluyor" "Installing packages"
if [ -z "$ROOT" ]; then
    export DEBIAN_FRONTEND=noninteractive
    # Kurulum ortamı paket kaynağı olarak hâlâ listede durur ve "apt-get update" bu yüzden hata verir;
    # kurucu onu en sonda kendisi kapatır, burada daha önce kapatılır.
    if [ -f /etc/apt/sources.list ] && grep -q '^deb cdrom:' /etc/apt/sources.list; then
        run sed -i 's/^deb cdrom:/# deb cdrom:/' /etc/apt/sources.list
    fi
    run apt-get update
    # shellcheck disable=SC2086
    run apt-get install -y --no-install-recommends $PACKAGES
fi

# ---------------------------------------------------------------- ikili dosya, kullanıcı, dizinler
say "RedoX VE kuruluyor" "Installing RedoX VE"
run install -D -m 0755 "$BINARY" "$ROOT/usr/local/bin/redox"
if [ -z "$ROOT" ]; then
    id redox >/dev/null 2>&1 || run useradd --system --home-dir /var/lib/redox --shell /usr/sbin/nologin redox
    run install -d -o redox -g redox -m 0711 /var/lib/redox
    # Yetkisiz konteynerler için kimlik aralığı ve sanal ağ kartı izni.
    grep -q '^redox:' /etc/subuid 2>/dev/null || run usermod --add-subuids 100000-165535 --add-subgids 100000-165535 redox
    run setfacl -m u:100000:x /var/lib/redox
fi
USERNET="$ROOT/etc/lxc/lxc-usernet"
run mkdir -p "$(dirname "$USERNET")"
for bridge in lxcbr0 vmbr0; do
    line="redox veth $bridge 20"
    if [ "$DRY_RUN" = 1 ]; then echo "+ $line >> $USERNET"; elif ! grep -qxF "$line" "$USERNET" 2>/dev/null; then echo "$line" >> "$USERNET"; fi
done

# ---------------------------------------------------------------- systemd birimi
# Panel yalnızca yerel adreste dinler; dışarıya TLS ile Caddy açar.
LISTEN=127.0.0.1:8006
[ "$TLS" = 1 ] && LISTEN=127.0.0.1:8005
UNIT="$ROOT/etc/systemd/system/redox.service"
if [ "$DRY_RUN" = 1 ]; then
    echo "+ write $UNIT (--addr $LISTEN)"
else
    mkdir -p "$(dirname "$UNIT")"
    sed "s|--addr 127.0.0.1:8006|--addr $LISTEN|" "$HERE/redox-host.service" > "$UNIT"
fi

# Dağıtık anahtarlar: ağ aygıtlarını kuran root birimi ve tanım dosyasını izleyen birim. qemu-bridge-helper,
# "include" edilen dosya yoksa bütün köprüleri reddeder; bu yüzden dosya önce oluşturulur.
VDS_CONF="$ROOT/etc/qemu/redox-vds.conf"
QEMU_CONF="$ROOT/etc/qemu/bridge.conf"
if [ "$DRY_RUN" = 1 ]; then
    echo "+ write redox-net.service redox-net.path $VDS_CONF; include in $QEMU_CONF"
else
    cp "$HERE/redox-net.service" "$HERE/redox-net.path" "$ROOT/etc/systemd/system/"
    # KSM birimi yalnızca kopyalanır: açmak yöneticinin kararıdır.
    cp "$HERE/redox-ksm.service" "$ROOT/etc/systemd/system/"
    mkdir -p "$ROOT/etc/qemu"
    [ -f "$VDS_CONF" ] || : > "$VDS_CONF"
    chmod 0644 "$VDS_CONF"
    # "include" satırı hedef sistemdeki yolu gösterir ($ROOT'suz).
    grep -qxF "include /etc/qemu/redox-vds.conf" "$QEMU_CONF" 2>/dev/null || echo "include /etc/qemu/redox-vds.conf" >> "$QEMU_CONF"
    chmod 0644 "$QEMU_CONF"
fi
[ -n "$ROOT" ] || [ ! -f /usr/lib/qemu/qemu-bridge-helper ] || run chmod u+s /usr/lib/qemu/qemu-bridge-helper

if [ "$TLS" = 1 ]; then
    CADDY="$ROOT/etc/caddy/Caddyfile"
    if [ "$DRY_RUN" = 1 ]; then
        echo "+ write $CADDY"
    else
        mkdir -p "$(dirname "$CADDY")"
        cat > "$CADDY" <<'EOF'
# RedoX VE: panel https://<sunucu adresi>:8006 üzerinde, kendinden imzalı sertifikayla.
# Alan adınız varsa bu dosyayı deploy/Caddyfile ile değiştirin (Let's Encrypt sertifikası).
{
	auto_https disable_redirects
	# Tarayıcı adrese IP ile bağlanınca sunucu adı göndermez; sertifika bu adla verilir.
	default_sni redox.local
}

https://:8006 {
	tls internal {
		on_demand
	}
	encode zstd gzip
	reverse_proxy 127.0.0.1:8005
}
EOF
    fi
fi

# ---------------------------------------------------------------- köprü (isteğe bağlı)
# /etc/network/interfaces içindeki arayüzü vmbr0 köprüsüne çevirir. Yalnızca ifupdown kullanan sistemlerde
# (Debian) çalışır; netplan ya da NetworkManager kullanan sistemde köprüyü KURULUM.md 9.1'e göre elle kurun.
bridge_interfaces() {
    # $1: dosya, $2: arayüz. Dönüştürülmüş içeriği standart çıktıya yazar; arayüz dosyada yoksa 1 döner.
    grep -Eq "^iface[[:space:]]+$2[[:space:]]+inet[[:space:]]" "$1" || return 1
    awk -v port="$2" '
        $1 == "auto" || $1 == "allow-hotplug" {
            if ($2 == port && NF == 2) { print "auto vmbr0"; next }
        }
        $1 == "iface" && $2 == port && $3 == "inet" {
            print "iface " port " inet manual"; print ""
            print "iface vmbr0 inet " $4
            print "    bridge-ports " port
            print "    bridge-stp off"
            print "    bridge-fd 0"
            next
        }
        $1 == "iface" && $2 == port { sub(port, "vmbr0") }
        { print }
    ' "$1"
}

if [ "$FROM_INSTALLER" = 1 ] && [ -z "$BRIDGE_PORT" ]; then
    # Kurulum ortamında: ilk yapılandırılmış fiziksel arayüz.
    BRIDGE_PORT=$(awk '$1 == "iface" && $2 != "lo" && $3 == "inet" { print $2; exit }' "$ROOT/etc/network/interfaces" 2>/dev/null || true)
fi
if [ -n "$BRIDGE_PORT" ]; then
    INTERFACES="$ROOT/etc/network/interfaces"
    if [ -f "$INTERFACES" ] && grep -q "vmbr0" "$INTERFACES"; then
        say "vmbr0 köprüsü zaten tanımlı" "Bridge vmbr0 is already defined"
    elif [ -f "$INTERFACES" ] && bridge_interfaces "$INTERFACES" "$BRIDGE_PORT" > "$INTERFACES.redox"; then
        say "$BRIDGE_PORT arayüzü vmbr0 köprüsüne bağlanıyor (eski dosya: interfaces.redox-once)" "Attaching $BRIDGE_PORT to bridge vmbr0 (old file: interfaces.redox-once)"
        if [ "$DRY_RUN" = 1 ]; then
            cat "$INTERFACES.redox"; rm -f "$INTERFACES.redox"
        else
            cp -p "$INTERFACES" "$INTERFACES.redox-once"
            mv "$INTERFACES.redox" "$INTERFACES"
        fi
    else
        rm -f "$INTERFACES.redox"
        say "UYARI: $BRIDGE_PORT /etc/network/interfaces içinde bulunamadı; köprü kurulmadı (konuklar NAT ile çalışır)" "WARNING: $BRIDGE_PORT not found in /etc/network/interfaces; no bridge was set up (guests use NAT)"
        BRIDGE_PORT=""
    fi
fi
if [ -n "$BRIDGE_PORT" ]; then
    CONF="$ROOT/etc/qemu/bridge.conf"
    run mkdir -p "$(dirname "$CONF")"
    if [ "$DRY_RUN" = 1 ]; then echo "+ allow vmbr0 >> $CONF"; elif ! grep -qxF "allow vmbr0" "$CONF" 2>/dev/null; then echo "allow vmbr0" >> "$CONF"; fi
    [ -n "$ROOT" ] || [ ! -f /usr/lib/qemu/qemu-bridge-helper ] || run chmod u+s /usr/lib/qemu/qemu-bridge-helper
fi

# ---------------------------------------------------------------- açılış ekranı ve ilk yönetici
# Konsol açılış ekranına panelin adresini yazar (her açılışta yenilenir).
BANNER="$ROOT/usr/local/sbin/redox-banner"
FIRSTBOOT="$ROOT/usr/local/sbin/redox-firstboot"
if [ "$DRY_RUN" = 1 ]; then
    echo "+ write $BANNER $FIRSTBOOT and their units"
else
    mkdir -p "$(dirname "$BANNER")" "$ROOT/etc/systemd/system"
    cat > "$BANNER" <<'EOF'
#!/bin/sh
# RedoX VE: konsol açılış ekranına panelin adresini yazar.
address=$(ip -4 -o route get 1.1.1.1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p')
[ -n "$address" ] || address=$(hostname -I 2>/dev/null | awk '{print $1}')
port=8006
scheme=https
# TLS'siz kurulumda panel yalnızca sunucunun kendisinden (ya da SSH tüneliyle) açılır.
if grep -q -- '--addr 127.0.0.1:8006' /etc/systemd/system/redox.service 2>/dev/null; then scheme=http; address=127.0.0.1; fi
{
    echo
    echo "  RedoX VE"
    echo
    if [ -n "$address" ]; then
        echo "  Panel / Web interface:  $scheme://$address:$port"
    else
        echo "  Ag adresi alinamadi / No network address yet."
    fi
    if [ -f /root/redox-ilk-parola.txt ]; then
        echo "  Ilk giris / First login: admin  (parola / password: /root/redox-ilk-parola.txt)"
    fi
    echo
} > /etc/issue
EOF
    chmod 0755 "$BANNER"
    cat > "$FIRSTBOOT" <<'EOF'
#!/bin/sh
# RedoX VE: ilk açılışta rastgele parolalı bir yönetici hesabı oluşturur. Parola yalnızca kökün okuyabildiği
# /root/redox-ilk-parola.txt dosyasına yazılır; ilk girişten sonra değiştirin ve dosyayı silin.
set -eu
users=/var/lib/redox/users.json
[ -s "$users" ] && exit 0
umask 077
password=$(head -c 18 /dev/urandom | base64 | tr -d '/+=' | cut -c1-20)
install -d -o redox -g redox -m 0711 /var/lib/redox
REDOX_PASSWORD="$password" runuser -u redox -- /usr/local/bin/redox user add admin --users "$users"
printf '%s\n' "$password" > /root/redox-ilk-parola.txt
EOF
    chmod 0755 "$FIRSTBOOT"
    cat > "$ROOT/etc/systemd/system/redox-firstboot.service" <<'EOF'
[Unit]
Description=RedoX VE ilk açılış: yönetici hesabı
Before=redox.service
ConditionPathExists=!/var/lib/redox/users.json

[Service]
Type=oneshot
ExecStart=/usr/local/sbin/redox-firstboot

[Install]
WantedBy=multi-user.target
EOF
    # Fiziksel konsolda (tty1) olağan oturum açma istemi yerine canlı durum ekranı durur; Enter ile oturum açılır.
    CONSOLE_SCHEME=https; [ "$TLS" = 1 ] || CONSOLE_SCHEME=http
    cat > "$ROOT/etc/systemd/system/redox-console.service" <<EOF
[Unit]
Description=RedoX VE konsol durum ekranı
After=redox-firstboot.service systemd-user-sessions.service plymouth-quit-wait.service
Conflicts=getty@tty1.service
Before=getty.target
ConditionPathExists=/dev/tty1

[Service]
ExecStart=/usr/local/bin/redox tty --listen $LISTEN --scheme $CONSOLE_SCHEME --port 8006 --state /var/lib/redox/state.json
StandardInput=tty
StandardOutput=tty
TTYPath=/dev/tty1
TTYReset=yes
TTYVHangup=yes
TTYVTDisallocate=yes
Restart=always
RestartSec=1

[Install]
WantedBy=multi-user.target
EOF
    cat > "$ROOT/etc/systemd/system/redox-banner.service" <<'EOF'
[Unit]
Description=RedoX VE konsol açılış ekranı
After=network-online.target redox-firstboot.service
Wants=network-online.target
Before=getty@tty1.service

[Service]
Type=oneshot
ExecStart=/usr/local/sbin/redox-banner

[Install]
WantedBy=multi-user.target
EOF
fi

# ---------------------------------------------------------------- hizmetleri aç
if [ -n "$ROOT" ]; then
    say "Sınama kipi: dosyalar $ROOT altına yazıldı" "Test mode: files were written under $ROOT"
    exit 0
fi
run systemctl enable redox.service redox-firstboot.service redox-banner.service redox-console.service redox-net.service redox-net.path
# tty1'i durum ekranı kullanır; öbür sanal konsollarda (Alt+F2) olağan oturum açma durur.
run systemctl disable getty@tty1.service
[ "$TLS" = 0 ] || run systemctl enable caddy.service
if [ "$FROM_INSTALLER" = 1 ]; then
    # Kurulum ortamında hizmetler başlatılmaz; ilk açılışta kendileri başlar.
    exit 0
fi
run systemctl daemon-reload
if [ ! -s /var/lib/redox/users.json ] && [ -t 0 ] && [ "$DRY_RUN" = 0 ]; then
    say "İlk yönetici hesabı (admin) için parola belirleyin" "Choose a password for the first administrator (admin)"
    runuser -u redox -- /usr/local/bin/redox user add admin --users /var/lib/redox/users.json
fi
run systemctl restart redox-firstboot.service
run systemctl restart redox.service
run systemctl restart redox-net.path
[ "$TLS" = 0 ] || run systemctl restart caddy.service
run systemctl restart redox-banner.service
[ "$DRY_RUN" = 1 ] || sed -n '/Panel/p' /etc/issue
[ -z "$BRIDGE_PORT" ] || say "Köprünün devreye girmesi için sunucuyu yeniden başlatın" "Reboot the server to bring the bridge up"
say "Kurulum bitti" "Installation finished"
