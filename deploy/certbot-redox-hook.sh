#!/bin/sh
# certbot yenileme kancası: yeni sertifikayı yönetim sunucusunun okuyabileceği yere kopyalar. Sunucu dosyanın
# değiştiğini görür; yeniden başlatmak gerekmez.
#   certbot certonly --standalone -d <ad> --deploy-hook /usr/local/lib/redox/certbot-redox-hook.sh
set -eu
: "${RENEWED_LINEAGE:?bu betik certbot --deploy-hook ile çalışır}"
install -d -m 0750 -o root -g redox /var/lib/redox-tls
install -m 0640 -o root -g redox "$RENEWED_LINEAGE/privkey.pem" /var/lib/redox-tls/tls.key
install -m 0644 -o root -g redox "$RENEWED_LINEAGE/fullchain.pem" /var/lib/redox-tls/tls.crt
