# Who opens the live socket (the chat's guest count): real people or bots?
# Reads an nginx access log (combined format, or nginx-proxy's "vhost" format with the host in
# front) and prints only totals, browser names and referring sites: no IP addresses, so the
# output can be shared.
#
#   docker logs --since 30m f0ckm-nginx 2>&1 | awk -f scripts/guest-check.awk
#   tail -n 200000 /var/log/nginx/access.log | awk -f scripts/guest-check.awk
#
# Only IPs that opened /api/notifications/ws count; logged-in users are in there too (the log
# doesn't know who is logged in), which only makes things look more human.

function is_ip(s) {
    return s ~ /^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$/ || (s ~ /:/ && s ~ /^[0-9a-fA-F:]+$/)
}
function net(ip,   p, n) {
    if (ip ~ /:/) { n = split(ip, p, ":"); return p[1] ":" p[2] ":" p[3] ":" p[4] "::/64" }
    split(ip, p, "."); return p[1] "." p[2] "." p[3] ".0/24"
}
function secs(line,   t, p) {
    # [19/Nov/2026:17:22:46 +0000] -> seconds within the month (good enough for spans)
    if (!match(line, /\[[0-9]+\/[A-Za-z]+\/[0-9]+:[0-9]+:[0-9]+:[0-9]+/)) return -1
    t = substr(line, RSTART + 1, RLENGTH - 1)
    split(t, p, /[\/:]/)
    return ((p[1] * 24 + p[4]) * 60 + p[5]) * 60 + p[6]
}
function browser(ua,   b, o) {
    b = ua ~ /[Hh]eadless|[Pp]uppeteer|[Pp]laywright|[Ss]elenium|[Pp]hantom/ ? "HEADLESS" : \
        ua ~ /[Bb]ot|[Cc]rawl|[Ss]pider/ ? "BOT" : \
        ua ~ /Edg\// ? "Edge" : ua ~ /OPR\// ? "Opera" : ua ~ /SamsungBrowser/ ? "Samsung" : \
        ua ~ /Firefox\// ? "Firefox" : ua ~ /Chrome\// ? "Chrome" : ua ~ /Safari\// ? "Safari" : "other"
    o = ua ~ /Android/ ? "Android" : ua ~ /iPhone|iPad/ ? "iOS" : ua ~ /Windows/ ? "Windows" : \
        ua ~ /Mac OS X/ ? "macOS" : ua ~ /Linux/ ? "Linux" : "?"
    return b " / " o
}
function bucket(n, lims, names,   i, l, nm) {
    split(lims, l, " "); split(names, nm, " ")
    for (i = 1; i in l; i++) if (n < l[i] + 0) return nm[i]
    return nm[i]
}

{
    ip = ""
    for (i = 1; i <= 3; i++) if (is_ip($i)) { ip = $i; break }
    if (ip == "") next
    nq = split($0, q, "\"")
    if (nq < 6) next
    split(q[2], r, " "); method = r[1]; path = r[2]; sub(/\?.*/, "", path)
    split(q[3], st, " "); status = st[1]
    ref = q[4]; ua = q[6]
    t = secs($0)

    if (!(ip in first) || t < first[ip]) first[ip] = t
    if (t > last[ip]) last[ip] = t
    lastua[ip] = ua

    if (path == "/api/notifications/ws") { ws[ip]++; next }
    if (path ~ /^\/(s|t|ca|b|a|c|memes)\//) { assets[ip]++; next }
    if (path ~ /^\/api\// || path ~ /\.[A-Za-z0-9]+$/) { api[ip]++; next }
    if (method == "GET" && status ~ /^(200|304)$/) {
        pages[ip]++
        if (ref != "-" && ref != "" && !(ip in entry)) {
            h = ref; sub(/^[a-z]+:\/\//, "", h); sub(/[\/:].*/, "", h)
            entry[ip] = h
        }
    }
}

END {
    for (ip in ws) {
        n++
        p = pages[ip] + 0
        pg[bucket(p, "1 2 5 20", "0 1 2-4 5-19 20+")]++
        span = last[ip] - first[ip]
        sp[bucket(span, "10 60 300 1800", "<10s 10s-1m 1-5m 5-30m 30m+")]++
        if (assets[ip] + 0 == 0) noassets++
        br[browser(lastua[ip])]++
        uas[lastua[ip]]++
        nets[net(ip)]++
        if (ip in entry) refs[entry[ip]]++; else refs["(none / direct)"]++
        if (first[ip] >= 0) arrive[int(first[ip] / 60)]++
    }
    if (!n) { print "No /api/notifications/ws requests in this log slice."; exit }

    printf "IPs that opened the live socket: %d\n\n", n
    print "Pages loaded per IP (people browse; rotating-IP scrapers load 1):"
    split("0 1 2-4 5-19 20+", o, " "); for (i = 1; i in o; i++) printf "  %-6s %5d  (%d%%)\n", o[i], pg[o[i]], 100 * pg[o[i]] / n
    print "\nTime between first and last request per IP:"
    split("<10s 10s-1m 1-5m 5-30m 30m+", o, " "); for (i = 1; i in o; i++) printf "  %-7s %5d  (%d%%)\n", o[i], sp[o[i]], 100 * sp[o[i]] / n
    printf "\nIPs that loaded no images/css/js at all: %d (%d%%)\n", noassets, 100 * noassets / n

    print "\nBrowser / OS:"
    cmd = "sort -rn | head -12"
    for (b in br) printf "  %5d  %s\n", br[b], b | cmd
    close(cmd)

    distinct = 0; for (u in uas) distinct++
    printf "\nDifferent user-agent strings: %d for %d IPs. Most common exact strings:\n", distinct, n
    cmd = "sort -rn | head -5"
    for (u in uas) printf "  %5d  %s\n", uas[u], substr(u, 1, 150) | cmd
    close(cmd)

    nn = 0; big = 0; for (x in nets) { nn++; if (nets[x] > big) big = nets[x] }
    printf "\nNetworks (/24 or /64): %d for %d IPs; the largest holds %d IPs\n", nn, n, big

    print "\nWhere they came from (referrer of their first page):"
    cmd = "sort -rn | head -10"
    for (x in refs) printf "  %5d  %s\n", refs[x], x | cmd
    close(cmd)

    print "\nNew IPs per minute (people arrive unevenly; a scraper keeps a steady beat):"
    cmd = "sort -n | tail -30 | cut -d' ' -f2-"
    for (m in arrive) printf "%d   %02d:%02d  %d\n", m, int((m % 1440) / 60), m % 60, arrive[m] | cmd
    close(cmd)
}
