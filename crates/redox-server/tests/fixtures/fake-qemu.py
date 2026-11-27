#!/usr/bin/env python3
"""Testler için sahte QEMU: gerçek QEMU gibi arka plana geçer, PID dosyası yazar
ve Unix soketi üzerinden QMP konuşur. Hiçbir şey sanallaştırmaz.

Konuk adı "inatci" ise system_powerdown isteğini yok sayar (zorla kapatma yolunu sınamak için).
"""
import base64
import json
import os
import socket
import subprocess
import sys
import threading
import time

args = sys.argv[1:]


def value(flag):
    return args[args.index(flag) + 1]


sock_path = value("-qmp").split(":", 1)[1].split(",")[0]
pid_path = value("-pidfile")
stubborn = value("-name") == "inatci"
# Sanal diskler sırayla virtio0, virtio1, ... adını alır (ilk sürücü asıl disktir).
disk_files = [a.split("file=", 1)[1].split(",if=")[0] for i, a in enumerate(args) if i and args[i - 1] == "-drive" and a.startswith("file=") and ",if=virtio" in a]
disk_path = disk_files[0]
devices = {"virtio%d" % i: path for i, path in enumerate(disk_files)}
qom = {}
blockstats = 0
# Bağlantı noktasına oturan ek diskler kendi adlarıyla gelir: file=YOL,if=none,id=xdN,...
for i, a in enumerate(args):
    if i and args[i - 1] == "-drive" and a.startswith("file=") and ",if=none,id=" in a:
        devices[a.split(",if=none,id=", 1)[1].split(",")[0]] = a.split("file=", 1)[1].split(",if=none")[0]
ports = [a.split("id=", 1)[1].split(",")[0] for a in args if a.startswith("pcie-root-port,")]
used_ports = set(a.split("bus=", 1)[1].split(",")[0] for a in args if ",bus=hp" in a)
# Çekirdek yuvaları: "-smp N,maxcpus=M". İlk N yuva doludur; kalanı çalışırken doldurulur.
smp = value("-smp").split(",")
cpus = [True] * int(smp[0])
cpus += [False] * (max([int(part.split("=")[1]) for part in smp[1:] if part.startswith("maxcpus=")] + [len(cpus)]) - len(cpus))
# Bellek yuvaları: "-m TABANM,slots=N,maxmem=ENFAZLAM". Komut satırındaki parçalar açılışta takılıdır.
mem = value("-m").split(",")
base_mb = int(mem[0].rstrip("M"))
max_mb = max([int(part.split("=")[1].rstrip("M")) for part in mem[1:] if part.startswith("maxmem=")] + [0])
mem_objects = {a.split("id=", 1)[1].split(",")[0]: int(a.split("size=", 1)[1].split(",")[0].rstrip("M")) for a in args if a.startswith("memory-backend-ram,")}
cold_dimms = [mem_objects[a.split("memdev=", 1)[1].split(",")[0]] for a in args if a.startswith("pc-dimm,")]
hot_dimms = {}
# Bir diskin kopyası başarısız olunca durdurulmayı bekleyen diğer işler.
waiting = set()

# Askıdan dönüş: bellek durumu dosyası yoksa ya da bozuksa gerçek QEMU gibi başlamadan çıkar.
incoming = None
# Canlı taşıma: "-incoming defer" ile açılan QEMU belleği sonradan (migrate-incoming) bekler.
deferred = "-incoming" in args and value("-incoming") == "defer"
if "-incoming" in args and not deferred:
    incoming = value("-incoming").split("exec:cat ", 1)[1].strip("'")
    if not os.path.exists(incoming) or open(incoming).read() != "bellek-durumu":
        sys.stderr.write("qemu-system-x86_64: load of migration failed: Invalid argument\n")
        sys.exit(1)

if "-daemonize" not in args:
    sys.stderr.write("fake-qemu: -daemonize bekleniyordu\n")
    sys.exit(2)

if os.fork():
    # Gerçek QEMU gibi: soket hazır olunca üst süreç 0 ile çıkar.
    for _ in range(200):
        if os.path.exists(sock_path):
            sys.exit(0)
        time.sleep(0.01)
    sys.stderr.write("fake-qemu: soket açılamadı\n")
    sys.exit(1)

os.setsid()
with open(pid_path, "w") as f:
    f.write(str(os.getpid()))
with open(sock_path + ".args", "w") as f:
    f.write("\n".join(args))


def hot(line):
    with open(sock_path + ".hot", "a") as out:
        out.write(line + "\n")


if incoming:
    with open(sock_path + ".resumed", "w") as f:
        f.write(incoming)
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(sock_path)
server.listen(8)
devnull = os.open(os.devnull, os.O_RDWR)
for fd in (0, 1, 2):
    os.dup2(devnull, fd)

paused = "-S" in args
# Canlı taşıma durumu. Gerçek QEMU'nun NBD ve taşıma akışı yerine satır tabanlı küçük bir protokol konuşulur:
#   KEY <anahtar>            iki uç aynı anahtarı bilmeli (gerçekte TLS el sıkışması)
#   EXPORT <ad> / DATA <n>   disk bağlantısı: diskin içeriği hedefin dosyasına yazılır
#   RAM <metin>              bellek bağlantısı: hedef "belleği" bir dosyaya yazar
mig = {"tls": {}, "exports": set(), "nodes": {}, "jobs": {}, "status": None, "error": None, "arrived": False, "listeners": []}


def mig_key(name):
    try:
        return open(os.path.join(mig["tls"][name], "keys.psk")).read().strip().split(":", 1)[1]
    except (KeyError, OSError, IndexError):
        return None


def mig_line(conn):
    data = b""
    while not data.endswith(b"\n"):
        chunk = conn.recv(1)
        if not chunk:
            break
        data += chunk
    return data.decode().strip()


def mig_listen(host, port, key, serve):
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind((host, port))
    listener.listen(8)
    mig["listeners"].append(listener)

    def loop():
        while True:
            try:
                conn, _ = listener.accept()
            except OSError:
                return
            try:
                # Anahtarı tutmayan bağlantı el sıkışmada düşer.
                if mig_line(conn) == "KEY %s" % key:
                    conn.sendall(b"OK\n")
                    serve(conn)
                else:
                    conn.close()
            except OSError:
                pass

    threading.Thread(target=loop, daemon=True).start()


def mig_serve_disk(conn):
    def run():
        try:
            name = mig_line(conn).split(" ", 1)[1]
            if name not in mig["exports"]:
                conn.sendall(b"ERR\n")
                return
            conn.sendall(b"OK\n")
            while True:
                head = mig_line(conn)
                if not head.startswith("DATA "):
                    return
                size, data = int(head.split(" ")[1]), b""
                while len(data) < size:
                    data += conn.recv(size - len(data))
                with open(devices[name], "wb") as out:
                    out.write(data)
                conn.sendall(b"DONE\n")
        except (OSError, IndexError, ValueError):
            pass

    threading.Thread(target=run, daemon=True).start()


def mig_serve_ram(conn):
    text = mig_line(conn)
    if text.startswith("RAM "):
        with open(sock_path + ".ram", "w") as out:
            out.write(text[4:])
        mig["arrived"] = True
        conn.sendall(b"OK\n")
    conn.close()


def mig_connect(uri_host, port, key):
    conn = socket.create_connection((uri_host, int(port)), timeout=5)
    conn.sendall(("KEY %s\n" % key).encode())
    if mig_line(conn) != "OK":
        conn.close()
        raise OSError("TLS handshake failed: The TLS connection was non-properly terminated")
    return conn


# Konuk ajanı kanalı: adı "ajanli" ile başlayan konukta ajan yanıt verir; diğerlerinde kanal sessizdir.
qga_path = next((a.split("path=", 1)[1].split(",")[0] for a in args if a.startswith("socket,id=qga0,")), None)
# Ajan: adı "ajanli" ile başlayan konuklarda ya da çalışma dizininde "ajan-var" dosyası varsa.
has_agent = value("-name").startswith("ajanli") or os.path.exists(os.path.join(os.path.dirname(sock_path), "ajan-var"))


# Ajanın yazdığı dosyalar: konuğun "kökü" bu dizindir. Üst dizini olmayan dosya gerçek konuktaki gibi açılamaz.
guest_root = (qga_path or "") + ".files"
guest_files = {}
tar_result = [0]


def agent(listener):
    while True:
        conn, _ = listener.accept()
        stream = conn.makefile("rw")
        try:
            for line in stream:
                if not has_agent:
                    continue
                request = json.loads(line)
                command = request["execute"]
                if command == "guest-sync":
                    # Önce eski bir yanıt kalıntısı: istemci kendi sayısını görene kadar atlamalı.
                    stream.write(json.dumps({"return": 1}) + "\n")
                    reply = {"return": request["arguments"]["id"]}
                else:
                    arguments = request.get("arguments", {})
                    with open(qga_path + ".log", "a") as out:
                        if command == "guest-exec":
                            out.write("guest-exec %s %s\n" % (arguments.get("path"), " ".join(arguments.get("arg", []))))
                        elif command != "guest-exec-status":
                            out.write(command + "\n")
                    if command == "guest-file-open":
                        host = guest_root + arguments["path"]
                        if not os.path.isdir(os.path.dirname(host)):
                            reply = {"error": {"class": "GenericError", "desc": "failed to open file '%s' (mode: 'wb'): No such file or directory" % arguments["path"]}}
                        else:
                            handle = 1000 + len(guest_files)
                            guest_files[handle] = open(host, "wb")
                            reply = {"return": handle}
                    elif command == "guest-file-write":
                        data = base64.b64decode(arguments["buf-b64"])
                        guest_files[arguments["handle"]].write(data)
                        # "Disk dolu": istenenden azı yazılır.
                        reply = {"return": {"count": len(data) - (1 if os.path.exists(qga_path + ".full") else 0), "eof": False}}
                    elif command == "guest-file-close":
                        guest_files.pop(arguments["handle"]).close()
                        reply = {"return": {}}
                    elif command == "guest-exec" and arguments.get("path") == "tar":
                        # tar -xpf ARŞİV -C DİZİN: konuğun kökü sayılan dizinde gerçekten açılır.
                        a = arguments["arg"]
                        tar_result[0] = subprocess.call(["tar", a[0], guest_root + a[1], a[2], guest_root + a[3]], stderr=subprocess.DEVNULL)
                        reply = {"return": {"pid": 20}}
                    elif command == "guest-exec" and arguments.get("path") == "rm":
                        os.remove(guest_root + arguments["arg"][-1])
                        reply = {"return": {"pid": 7}}
                    elif command == "guest-exec-status" and arguments.get("pid") == 20:
                        reply = {"return": {"exited": True, "exitcode": tar_result[0], "out-data": "" if tar_result[0] == 0 else "dGFyOiBoYXRhCg=="}}
                    elif command == "guest-exec":
                        # Çalıştırılan programın yolunda "basarisiz" geçiyorsa program hata koduyla biter.
                        reply = {"return": {"pid": 3 if "basarisiz" in arguments.get("path", "") else 7}}
                    elif command == "guest-exec-status":
                        failed = arguments.get("pid") == 3
                        reply = {"return": {"exited": True, "exitcode": 3 if failed else 0, "out-data": "dmVyaXRhYmFuaSBtZXNndWwK" if failed else "dGFtYW0K"}}
                    elif command == "guest-fsfreeze-freeze":
                        reply = {"error": {"class": "GenericError", "desc": "freeze refused"}} if os.path.exists(qga_path + ".nofreeze") else {"return": 2}
                    elif command == "guest-fsfreeze-thaw":
                        reply = {"return": 2}
                    elif command == "guest-network-get-interfaces":
                        reply = {"return": [
                            {"name": "lo", "ip-addresses": [{"ip-address-type": "ipv4", "ip-address": "127.0.0.1", "prefix": 8}]},
                            {"name": "eth0", "ip-addresses": [{"ip-address-type": "ipv6", "ip-address": "fe80::1", "prefix": 64},
                                                              {"ip-address-type": "ipv4", "ip-address": "10.0.2.15", "prefix": 24}]}]}
                    elif command == "guest-get-fsinfo":
                        reply = {"return": [
                            {"name": "vda1", "mountpoint": "/", "type": "ext4", "used-bytes": 9 << 30, "total-bytes": 10 << 30},
                            {"name": "proc", "mountpoint": "/proc", "type": "proc"}]}
                    elif command == "guest-get-vcpus":
                        # Sonradan eklenen çekirdekler konukta kapalı görünür; ajan açana kadar.
                        opened = os.path.exists(qga_path + ".cpus")
                        cold = int(smp[0])
                        reply = {"return": [{"logical-id": i, "online": i < cold or opened, "can-offline": i > 0} for i in range(sum(cpus))]}
                    elif command == "guest-get-memory-block-info":
                        reply = {"return": {"size": 128 << 20}}
                    elif command == "guest-get-memory-blocks":
                        # Sonradan takılan bellek konukta kapalı görünür; ajan açana kadar.
                        opened = os.path.exists(qga_path + ".mem")
                        cold = (base_mb + sum(cold_dimms)) // 128
                        reply = {"return": [{"phys-index": i, "online": i < cold or opened, "can-offline": i > 0} for i in range(cold + sum(hot_dimms.values()) // 128)]}
                    elif command == "guest-set-memory-blocks":
                        open(qga_path + ".mem", "w").write(" ".join(str(b["phys-index"]) for b in arguments["mem-blks"]))
                        reply = {"return": [{"phys-index": b["phys-index"], "response": "success"} for b in arguments["mem-blks"]]}
                    elif command == "guest-set-vcpus":
                        open(qga_path + ".cpus", "w").write(" ".join(str(c["logical-id"]) for c in arguments["vcpus"]))
                        reply = {"return": len(arguments["vcpus"])}
                    elif command == "guest-get-osinfo":
                        reply = {"return": {"name": "Debian GNU/Linux", "pretty-name": "Debian GNU/Linux 12 (bookworm)", "version": "12"}}
                    else:
                        reply = {"error": {"class": "CommandNotFound", "desc": "unknown"}}
                stream.write(json.dumps(reply) + "\n")
                stream.flush()
        except (OSError, ValueError, KeyError):
            pass
        finally:
            conn.close()


if qga_path:
    qga = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    qga.bind(qga_path)
    qga.listen(8)
    threading.Thread(target=agent, args=(qga,), daemon=True).start()


# Canlı konsol soketi: gerçek VNC sunucusu gibi karşılamayı yollar, sonra gelen her baytı geri gönderir.
vnc_path = next((args[i + 1].split("unix:", 1)[1] for i, a in enumerate(args[:-1]) if a == "-vnc" and args[i + 1].startswith("unix:")), None)


def vnc(listener):
    def serve(conn):
        try:
            conn.sendall(b"RFB 003.008\n")
            while True:
                data = conn.recv(65536)
                if not data:
                    break
                conn.sendall(data)
        except OSError:
            pass
        finally:
            conn.close()

    while True:
        conn, _ = listener.accept()
        threading.Thread(target=serve, args=(conn,), daemon=True).start()


if vnc_path:
    screen = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    screen.bind(vnc_path)
    screen.listen(8)
    threading.Thread(target=vnc, args=(screen,), daemon=True).start()


def shutdown():
    for path in (sock_path, qga_path, vnc_path):
        try:
            os.unlink(path)
        except (OSError, TypeError):
            pass
    os._exit(0)


while True:
    conn, _ = server.accept()
    stream = conn.makefile("rw")

    def send(obj):
        stream.write(json.dumps(obj) + "\n")
        stream.flush()

    try:
        send({"QMP": {"version": {"qemu": {"major": 9}}, "capabilities": []}})
        for line in stream:
            request = json.loads(line)
            command = request["execute"]
            arguments = request.get("arguments", {})
            if command == "query-blockstats":
                # Her sorguda artan sayaçlar: 1 MiB okuma, 2 MiB yazma.
                blockstats += 1
                send({"return": [{"device": name, "stats": {"rd_bytes": blockstats << 20, "wr_bytes": blockstats << 21}} for name in list(devices)[:1]]})
            elif command == "query-block":
                send({"return": [{"device": "ide2-cd0", "inserted": {"file": "/başka/dosya.iso"}}] + [{"device": name, "inserted": {"file": path}} for name, path in devices.items()]})
            elif command == "transaction":
                if all(a["type"] == "blockdev-snapshot-internal-sync" for a in arguments["actions"]):
                    # İç anlık görüntü: sahte qemu-img ile aynı yan dosyaya (<imaj>.snaps) yazılır.
                    if any(a["data"]["device"] not in devices for a in arguments["actions"]):
                        send({"error": {"class": "DeviceNotFound", "desc": "Cannot find device"}})
                        continue
                    for a in arguments["actions"]:
                        with open(devices[a["data"]["device"]] + ".snaps", "a") as out:
                            out.write(a["data"]["name"] + "\n")
                    hot("disk-snapshot %s x%d" % (arguments["actions"][0]["data"]["name"], len(arguments["actions"])))
                    send({"return": {}})
                    continue
                jobs = [a["data"] for a in arguments["actions"]]
                for j in jobs:
                    if "speed" in j:
                        hot("backup-speed %d" % j["speed"])
                if any(a["type"] != "drive-backup" for a in arguments["actions"]) or any(j["device"] not in devices or not j.get("job-id") for j in jobs):
                    send({"error": {"class": "DeviceNotFound", "desc": "Cannot find device"}})
                    continue
                failing = [j for j in jobs if os.path.exists(j["target"] + ".fail")]
                # Gerçek QEMU, kopyalanacak verisi az olan işin "bitti" olayını başlama yanıtından önce yollar.
                early = value("-name").startswith("hizli") and not failing
                if early:
                    for j in jobs:
                        with open(devices[j["device"]], "rb") as src, open(j["target"], "wb") as dst:
                            dst.write(src.read())
                        send({"timestamp": {"seconds": 2, "microseconds": 0}, "event": "BLOCK_JOB_COMPLETED",
                              "data": {"device": j["job-id"], "len": 4, "offset": 4, "speed": 0, "type": "backup"}})
                    send({"return": {}})
                    continue
                send({"return": {}})
                # Araya ilgisiz bir olay girer; istemci yalnızca kendi işlerini izlemeli.
                send({"timestamp": {"seconds": 1, "microseconds": 0}, "event": "BLOCK_JOB_COMPLETED", "data": {"device": "baska-is"}})
                if failing:
                    # Biri hata verir; diğerleri istemci durdurana kadar sürer.
                    waiting.update(j["job-id"] for j in jobs if j not in failing)
                    for j in failing:
                        send({"timestamp": {"seconds": 1, "microseconds": 0}, "event": "BLOCK_JOB_COMPLETED",
                              "data": {"device": j["job-id"], "error": "No space left on device"}})
                else:
                    for j in jobs:
                        with open(devices[j["device"]], "rb") as src, open(j["target"], "wb") as dst:
                            dst.write(src.read())
                        send({"timestamp": {"seconds": 2, "microseconds": 0}, "event": "BLOCK_JOB_COMPLETED",
                              "data": {"device": j["job-id"], "len": 4, "offset": 4, "speed": 0, "type": "backup"}})
            elif command == "block-job-cancel" and arguments["device"] in mig["jobs"]:
                mig["jobs"].pop(arguments["device"])
                hot("block-job-cancel %s" % arguments["device"])
                send({"return": {}})
            elif command == "block-job-cancel":
                if arguments["device"] in waiting:
                    waiting.discard(arguments["device"])
                    send({"return": {}})
                    send({"timestamp": {"seconds": 2, "microseconds": 0}, "event": "BLOCK_JOB_CANCELLED", "data": {"device": arguments["device"]}})
                else:
                    send({"error": {"class": "DeviceNotActive", "desc": "No active block job"}})
            elif command == "drive-backup":
                if arguments["device"] != "virtio0" or not arguments.get("job-id"):
                    send({"error": {"class": "DeviceNotFound", "desc": "Cannot find device"}})
                elif os.path.exists(arguments["target"] + ".fail"):
                    send({"return": {}})
                    send({"timestamp": {"seconds": 1, "microseconds": 0}, "event": "BLOCK_JOB_COMPLETED",
                          "data": {"device": arguments["job-id"], "error": "No space left on device"}})
                else:
                    send({"return": {}})
                    # Araya ilgisiz bir olay girer; istemci yalnızca kendi işini izlemeli.
                    send({"timestamp": {"seconds": 1, "microseconds": 0}, "event": "BLOCK_JOB_COMPLETED", "data": {"device": "baska-is"}})
                    with open(disk_path, "rb") as src, open(arguments["target"], "wb") as dst:
                        dst.write(src.read())
                    send({"timestamp": {"seconds": 2, "microseconds": 0}, "event": "BLOCK_JOB_COMPLETED",
                          "data": {"device": arguments["job-id"], "len": 4, "offset": 4, "speed": 0, "type": "backup"}})
            elif command == "migrate-set-parameters":
                mig["params"] = arguments
                send({"return": {}})
            elif command == "nbd-server-start":
                key = mig_key(arguments.get("tls-creds"))
                if key is None:
                    send({"error": {"class": "GenericError", "desc": "No TLS credentials with id '%s'" % arguments.get("tls-creds")}})
                else:
                    try:
                        mig_listen(arguments["addr"]["data"]["host"], int(arguments["addr"]["data"]["port"]), key, mig_serve_disk)
                        hot("nbd-server-start %s" % arguments["addr"]["data"]["port"])
                        send({"return": {}})
                    except OSError as e:
                        send({"error": {"class": "GenericError", "desc": "Failed to bind socket: %s" % e}})
            elif command == "nbd-server-add":
                if arguments["device"] in devices and arguments.get("writable"):
                    mig["exports"].add(arguments["device"])
                    hot("nbd-server-add %s" % arguments["device"])
                    send({"return": {}})
                else:
                    send({"error": {"class": "GenericError", "desc": "Cannot find device=%s nor node-name=%s" % (arguments["device"], arguments["device"])}})
            elif command == "nbd-server-stop":
                for listener in mig["listeners"][:1]:
                    listener.close()
                hot("nbd-server-stop")
                send({"return": {}})
            elif command == "migrate-incoming":
                key = mig_key(mig.get("params", {}).get("tls-creds"))
                if not deferred or key is None:
                    send({"error": {"class": "GenericError", "desc": "'-incoming' was not specified on the command line"}})
                else:
                    host, port = arguments["uri"][4:].rsplit(":", 1)
                    try:
                        mig_listen(host, int(port), key, mig_serve_ram)
                        hot("migrate-incoming %s" % port)
                        send({"return": {}})
                    except OSError as e:
                        send({"error": {"class": "GenericError", "desc": "Failed to bind socket: %s" % e}})
            elif command == "blockdev-add":
                key = mig_key(arguments.get("tls-creds"))
                try:
                    link = mig_connect(arguments["server"]["host"], arguments["server"]["port"], key)
                    link.sendall(("EXPORT %s\n" % arguments["export"]).encode())
                    if mig_line(link) != "OK":
                        raise OSError("export '%s' not present" % arguments["export"])
                    mig["nodes"][arguments["node-name"]] = link
                    hot("blockdev-add nbd %s %s" % (arguments["node-name"], arguments["export"]))
                    send({"return": {}})
                except OSError as e:
                    send({"error": {"class": "GenericError", "desc": "Failed to linkect to '%s:%s': %s" % (arguments["server"]["host"], arguments["server"]["port"], e)}})
            elif command == "blockdev-del":
                link = mig["nodes"].pop(arguments["node-name"], None)
                if link:
                    link.close()
                hot("blockdev-del %s" % arguments["node-name"])
                send({"return": {}})
            elif command == "blockdev-mirror":
                link = mig["nodes"].get(arguments["target"])
                if link is None or arguments["device"] not in devices or arguments.get("copy-mode") != "write-blocking":
                    send({"error": {"class": "GenericError", "desc": "Cannot find device='%s' nor node-name='%s'" % (arguments["target"], arguments["target"])}})
                else:
                    data = open(devices[arguments["device"]], "rb").read()
                    link.sendall(("DATA %d\n" % len(data)).encode() + data)
                    mig_line(link)
                    # "aynasiz" işaretiyle ayna hemen düşer (hedefe yazılamıyor gibi).
                    if not os.path.exists(sock_path + ".nomirror"):
                        mig["jobs"][arguments["job-id"]] = arguments["device"]
                    hot("blockdev-mirror %s %s" % (arguments["device"], arguments["target"]))
                    send({"return": {}})
            elif command == "drive-mirror":
                # Çalışırken depolama taşıma: disk yeni dosyasına kopyalanır, iş "hazır" bekler.
                if arguments["device"] not in devices or arguments.get("copy-mode") != "write-blocking" or arguments.get("format") != "qcow2":
                    send({"error": {"class": "GenericError", "desc": "Cannot find device='%s'" % arguments["device"]}})
                elif os.path.exists(sock_path + ".nospace"):
                    send({"error": {"class": "GenericError", "desc": "Could not create file: No space left on device"}})
                else:
                    with open(devices[arguments["device"]], "rb") as src, open(arguments["target"], "wb") as dst:
                        dst.write(src.read())
                    if not os.path.exists(sock_path + ".nomirror"):
                        mig["jobs"][arguments["job-id"]] = arguments["device"]
                        mig.setdefault("moves", {})[arguments["job-id"]] = arguments["target"]
                    hot("drive-mirror %s %s" % (arguments["device"], arguments["target"]))
                    send({"return": {}})
            elif command == "block-job-complete":
                job = arguments["device"]
                # "nopivot-<aygıt>" işaretiyle o diskin geçişi başarısız olur.
                if job not in mig.get("moves", {}) or os.path.exists("%s.nopivot-%s" % (sock_path, mig["jobs"].get(job))):
                    send({"error": {"class": "GenericError", "desc": "The active block job '%s' cannot be completed" % job}})
                else:
                    devices[mig["jobs"].pop(job)] = mig["moves"].pop(job)
                    hot("block-job-complete %s" % job)
                    send({"return": {}})
            elif command == "migrate_cancel":
                hot("migrate_cancel")
                send({"return": {}})
            elif command == "migrate" and arguments["uri"].startswith("tcp:"):
                host, port = arguments["uri"][4:].rsplit(":", 1)
                if os.path.exists(sock_path + ".nomigrate"):
                    mig["status"], mig["error"] = "failed", "load of migration failed: Invalid argument"
                else:
                    try:
                        link = mig_connect(host, port, mig_key(mig.get("params", {}).get("tls-creds")))
                        link.sendall(("RAM %s\n" % value("-name")).encode())
                        mig_line(link)
                        link.close()
                        # "aynakopar" işaretiyle ayna bellek taşınırken düşer.
                        if os.path.exists(sock_path + ".dropmirror"):
                            mig["jobs"].clear()
                        mig["status"], paused = "completed", True
                    except OSError as e:
                        mig["status"], mig["error"] = "failed", str(e)
                hot("migrate %s" % port)
                send({"return": {}})
            elif command == "query-migrate" and mig["status"]:
                reply = {"status": mig["status"], "downtime": 7, "total-time": 90}
                if mig["error"]:
                    reply["error-desc"] = mig["error"]
                send({"return": reply})
            elif command == "migrate":
                # "askisiz" adlı konukta durum yazılamaz (dolu disk gibi).
                target = arguments["uri"].split("exec:cat > ", 1)[1].strip("'")
                if value("-name") != "askisiz":
                    with open(target, "w") as out:
                        out.write("bellek-durumu")
                send({"return": {}})
            elif command == "query-migrate":
                send({"return": {"status": "failed" if value("-name") == "askisiz" else "completed"}})
            elif command == "human-monitor-command" and arguments["command-line"].split(" ")[0] in ("drive_add", "drive_del"):
                words = arguments["command-line"].split(" ")
                if words[0] == "drive_del":
                    devices.pop(words[1], None)
                    hot("drive_del " + words[1])
                    send({"return": ""})
                else:
                    spec = words[2]
                    path = spec.split("file=", 1)[1].split(",if=none")[0]
                    name = spec.split(",id=", 1)[1].split(",")[0]
                    if name in devices:
                        # Gerçek QEMU gibi: aynı adla ikinci sürücü tanıtılamaz.
                        send({"return": "Duplicate ID '%s' for drive\r\n" % name})
                    elif not os.path.exists(path):
                        send({"return": "Could not open '%s': No such file or directory\r\n" % path})
                    else:
                        devices[name] = path
                        hot("drive_add %s %s" % (name, spec))
                        send({"return": "OK\r\n"})
            elif command in ("device_add", "netdev_add", "netdev_del", "device_del"):
                if command == "device_add":
                    bus = arguments.get("bus", "")
                    if os.path.exists(sock_path + ".nohot"):
                        send({"error": {"class": "GenericError", "desc": "Hot plug refused"}})
                        continue
                    if arguments["driver"] == "pc-dimm":
                        size = mem_objects.get(arguments.get("memdev"))
                        slot = arguments.get("slot", -1)
                        if not max_mb:
                            send({"error": {"class": "GenericError", "desc": "memory hotplug is not enabled: missing acpi-dev link"}})
                        elif size is None:
                            send({"error": {"class": "GenericError", "desc": "Property 'pc-dimm.memdev' can't find value '%s'" % arguments.get("memdev")}})
                        elif slot in hot_dimms or slot < len(cold_dimms):
                            send({"error": {"class": "GenericError", "desc": "slot %d is busy" % slot}})
                        elif base_mb + sum(cold_dimms) + sum(hot_dimms.values()) + size > max_mb:
                            send({"error": {"class": "GenericError", "desc": "not enough space, currently in use of total space for memory devices"}})
                        else:
                            hot_dimms[slot] = size
                            hot("device_add pc-dimm %s %s slot %d" % (arguments["id"], arguments["memdev"], slot))
                            send({"return": {}})
                        continue
                    if arguments["driver"].endswith("-cpu"):
                        core = arguments.get("core-id", -1)
                        if not 0 <= core < len(cpus) or cpus[core]:
                            send({"error": {"class": "GenericError", "desc": "CPU[%d] with APIC ID %d exists" % (core, core)}})
                            continue
                        cpus[core] = True
                        hot("device_add cpu %s core %d" % (arguments["id"], core))
                        send({"return": {}})
                        continue
                    if bus not in ports:
                        send({"error": {"class": "GenericError", "desc": "Bus '%s' not found" % bus}})
                        continue
                    if bus in used_ports:
                        send({"error": {"class": "GenericError", "desc": "PCI: slot 0 function 0 not available for %s" % arguments["driver"]}})
                        continue
                    used_ports.add(bus)
                    hot("device_add %s %s %s %s" % (arguments["driver"], arguments["id"], bus, arguments.get("drive") or arguments.get("netdev")))
                elif command == "netdev_add":
                    hot("netdev_add %s %s %s" % (arguments["type"], arguments["id"], arguments.get("br", "-")))
                else:
                    hot("%s %s" % (command, arguments["id"]))
                send({"return": {}})
            elif command == "object-add" and arguments["qom-type"] == "tls-creds-psk":
                mig["tls"][arguments["id"]] = arguments["dir"]
                hot("object-add tls-creds-psk %s %s" % (arguments["id"], arguments["endpoint"]))
                send({"return": {}})
            elif command == "object-add":
                mem_objects[arguments["id"]] = arguments["size"] >> 20
                hot("object-add %s %s %d" % (arguments["qom-type"], arguments["id"], arguments["size"] >> 20))
                send({"return": {}})
            elif command == "object-del":
                mig["tls"].pop(arguments["id"], None)
                mem_objects.pop(arguments["id"], None)
                hot("object-del %s" % arguments["id"])
                send({"return": {}})
            elif command == "query-hotpluggable-cpus":
                # Gerçek QEMU gibi: en yüksek numaralı yuva başta; dolu yuvaların "qom-path" alanı vardır.
                slots = []
                for core in reversed(range(len(cpus))):
                    slot = {"props": {"core-id": core, "thread-id": 0, "socket-id": 0}, "vcpus-count": 1, "type": "host-x86_64-cpu"}
                    if cpus[core]:
                        slot["qom-path"] = "/machine/unattached/device[%d]" % core
                    slots.append(slot)
                send({"return": slots})
            elif command == "human-monitor-command":
                verb, _, name = arguments["command-line"].partition(" ")
                snaps_path = disk_path + ".snaps"
                snaps = open(snaps_path).read().split() if os.path.exists(snaps_path) else []
                text = ""
                if verb == "savevm":
                    snaps.append(name)
                elif verb in ("loadvm", "delvm") and name not in snaps:
                    text = "Error: Snapshot '%s' does not exist in one or more devices\r\n" % name
                elif verb == "delvm":
                    snaps.remove(name)
                elif verb != "loadvm":
                    text = "unknown command: '%s'\r\n" % verb
                with open(snaps_path, "w") as out:
                    out.write("".join(n + "\n" for n in snaps))
                send({"return": text})
            elif command == "screendump":
                # Eski QEMU gibi davran: "format" bilinmez, yalnızca PPM yazılır (yedek yol sınansın).
                if "format" in arguments:
                    send({"error": {"class": "GenericError", "desc": "Parameter 'format' is unexpected"}})
                else:
                    with open(arguments["filename"], "wb") as out:
                        out.write(b"P6\n4 2\n255\n" + bytes(range(24)))
                    send({"return": {}})
            elif command == "send-key":
                with open(sock_path + ".keys", "a") as out:
                    out.write("+".join(k["data"] for k in arguments["keys"]) + "\n")
                send({"return": {}})
            elif command == "query-status":
                status = "inmigrate" if deferred and not mig["arrived"] else "postmigrate" if mig["status"] == "completed" and paused else "paused" if paused else "running"
                send({"return": {"status": status, "running": not paused}})
            elif command == "stop":
                paused = True
                # Gerçek QEMU yanıttan önce olay gönderebilir; istemci bunu atlamalı.
                send({"timestamp": {"seconds": 1, "microseconds": 0}, "event": "STOP"})
                send({"return": {}})
            elif command == "query-block-jobs":
                send({"return": [{"device": job, "ready": True, "offset": 4, "len": 4, "type": "mirror"} for job in mig["jobs"]]})
            elif command == "block-stream":
                # Arkalıktaki veri diske akar ve bağ kalkar (sahte imajda arkalık "bagli:<yol>" satırıdır).
                path = devices.get(arguments["device"])
                if path is None:
                    send({"error": {"class": "DeviceNotFound", "desc": "Cannot find device"}})
                    continue
                with open(path, "rb") as f:
                    content = f.read()
                if content.startswith(b"bagli:"):
                    with open(content[6:].decode(), "rb") as base, open(path, "wb") as out:
                        out.write(base.read())
                hot("block-stream %s" % arguments["device"])
                send({"return": {}})
            elif command == "qom-set":
                qom[arguments["property"]] = arguments["value"]
                hot("qom-set %s=%s" % (arguments["property"], arguments["value"]))
                send({"return": {}})
            elif command == "qom-get":
                if arguments["property"] == "guest-stats":
                    # Sınama, konuğun bildireceği ölçümü yan dosyaya yazar: "<toplam MB> <kullanılabilir MB>".
                    try:
                        # Gerçek QEMU bildirilmeyen değeri 2^64-1 olarak verir; "yok" bunu canlandırır.
                        missing = 18446744073709551615
                        total, available = [missing if x == "yok" else int(x) << 20 for x in open(sock_path + ".memstats").read().split()]
                        send({"return": {"last-update": 1700000000, "stats": {"stat-total-memory": total, "stat-available-memory": available, "stat-swap-in": missing}}})
                    except (OSError, ValueError):
                        send({"return": {"last-update": 0, "stats": {"stat-total-memory": 18446744073709551615, "stat-available-memory": 18446744073709551615}}})
                else:
                    send({"return": qom.get(arguments["property"], 0)})
            elif command == "balloon":
                if any("virtio-balloon" in a for a in args):
                    hot("balloon %d" % arguments["value"])
                    send({"return": {}})
                else:
                    send({"error": {"class": "DeviceNotActive", "desc": "No balloon device has been activated"}})
            elif command == "cont":
                paused = False
                send({"timestamp": {"seconds": 1, "microseconds": 0}, "event": "RESUME"})
                send({"return": {}})
            elif command in ("qmp_capabilities", "system_reset"):
                send({"return": {}})
            elif command == "system_powerdown":
                send({"return": {}})
                if not stubborn:
                    shutdown()
            elif command == "quit":
                send({"return": {}})
                shutdown()
            else:
                send({"error": {"class": "CommandNotFound", "desc": "The command %s has not been found" % command}})
    except (OSError, ValueError, KeyError):
        pass
    finally:
        conn.close()
