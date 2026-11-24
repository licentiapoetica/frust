#!/usr/bin/awk -f
# log-stats.awk: what clients do, read from f0ckm's own log, nginx access logs and Anubis' log, to
# see how real guests behave next to scrapers (scraper_guard limits). Plain POSIX awk (mawk, gawk,
# busybox). Every line is recognised by itself, so logs can be mixed and concatenated in any order;
# gzipped ones go through zcat. LC_ALL=C: in other locales awk reads "0.81" as 0 (decimal comma).
#
#   zcat -f /var/log/nginx/access.log* | LC_ALL=C awk -f scripts/log-stats.awk
#   docker compose logs --no-color f0ckm f0ckm-nginx anubis | LC_ALL=C awk -f scripts/log-stats.awk
#   zcat -f logs/* | LC_ALL=C awk -f scripts/log-stats.awk > stats.txt
#
# Memory grows with distinct (client, item) pairs; for many GB of logs, run it per month.
#
# Formats:
#   nginx  `combined` or nginx-proxy's `vhost` (host first): IP, time, request, status, referer, UA
#   f0ckm  "<RFC3339 time>  INFO  1.23ms  GET  200  /path" (no IP: site-wide numbers only)
#   anubis JSON lines with "msg"
# Anything else is counted as unparsed (a few masked samples are shown).
#
# Clients are keyed like scraper_guard: IPv4 address, IPv6 /64. The output never shows full IPs:
# clients are numbered (c1, c2, ...) with their /16 (IPv4) or first two groups (IPv6).
# "things" = distinct items opened + distinct original files (/b/) downloaded, as scraper_guard counts.
#
# Variables (awk -v name=value): TOP=25 rows in top lists, SAMPLES=5 unparsed samples.

BEGIN {
    if (TOP == "") TOP = 25
    if (SAMPLES == "") SAMPLES = 5
    split("Jan Feb Mar Apr May Jun Jul Aug Sep Oct Nov Dec", mn, " ")
    for (i = 1; i <= 12; i++) MON[mn[i]] = sprintf("%02d", i)
    nthr = split("50 100 250 500 1000 1500 3000 5000", THR, " ")
    ncl = 0
}

# ── helpers ──────────────────────────────────────────────────────────────────

function ipkey(ip,    n, p, i, out, head) {
    if (ip ~ /^::ffff:[0-9.]+$/) sub(/^::ffff:/, "", ip)
    if (index(ip, ":") == 0) return ip
    # IPv6: the /64 (first four groups; "::" inside them fills with zeros)
    head = ip
    if (index(head, "::")) head = substr(head, 1, index(head, "::") - 1)
    n = split(head, p, ":")
    out = ""
    for (i = 1; i <= 4; i++) out = out (i > 1 ? ":" : "") (i <= n && p[i] != "" ? tolower(p[i]) : "0")
    return out "::/64"
}

function masked(k,    p, n) {
    if (index(k, ":")) { n = split(k, p, ":"); return p[1] ":" p[2] ":x::/64" }
    n = split(k, p, ".")
    return p[1] "." p[2] ".x.x"
}

function client(k) {
    if (!(k in CID)) { CID[k] = "c" (++ncl); CMASK[CID[k]] = masked(k) }
    return CID[k]
}

function mask_ips(s) {
    gsub(/[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+/, "x.x.x.x", s)
    gsub(/[0-9a-fA-F]*:[0-9a-fA-F]*:[0-9a-fA-F]*:[0-9a-fA-F:]*/, "x:x::x", s)
    return s
}

function is_id(s) {
    if (s ~ /^[0-9]+$/) return 1
    return length(s) == 11 && s ~ /^[A-Za-z0-9_-]+$/
}

# Kind of request: file (original /b/), item, thumb, asset (/s/), avatar, api, ajax, page, other.
# Sets THING to the counted key for files and items, "" otherwise.
function kind(path,    q, n, seg, last, prev) {
    THING = ""
    q = index(path, "?"); if (q) path = substr(path, 1, q - 1)
    if (path ~ /^\/b\//) { THING = "b:" path; return "file" }
    if (path ~ /^\/(t|ca)\//) return "thumb"
    if (path ~ /^\/s\//) return "asset"
    if (path ~ /^\/(a|c|memes)\//) return "avatar"
    if (path ~ /^\/(ajax\/item|api\/v2\/item|api\/comments)\/[A-Za-z0-9_-]+\/?$/) {
        n = split(path, seg, "/"); last = seg[n] == "" ? seg[n - 1] : seg[n]
        if (is_id(last)) { THING = "i:" last; return "item" }
    }
    if (path ~ /^\/api\//) return "api"
    if (path ~ /^\/ajax\//) return "ajax"
    if (path ~ /^\/(login|register|logout|settings|admin|mod|notifications|messages)(\/|$)/) return "page"
    if (path ~ /^\/user\/[^\/]+\/?$/) return "page"
    sub(/\/$/, "", path)
    n = split(path, seg, "/")
    last = seg[n]; prev = (n > 1) ? seg[n - 1] : ""
    if (n >= 2 && prev != "p" && is_id(last)) { THING = "i:" last; return "item" }
    if (path ~ /\.(ico|png|gif|txt|json|js|xml|webmanifest)$/) return "other"
    return "page"
}

function uaclass(ua,    l) {
    l = tolower(ua)
    if (ua == "" || ua == "-") return "empty"
    if (l ~ /headless|phantom|puppeteer|playwright|selenium/) return "headless"
    if (l ~ /python|curl|wget|go-http|java\/|okhttp|axios|node-fetch|scrapy|httpx|aiohttp|libwww|perl|ruby|php|rust|reqwest|gallery-dl|yt-dlp|jdownloader/) return "tool"
    if (l ~ /bot|crawl|spider|slurp|facebookexternalhit|preview|embed|fetcher|archiver/) return "bot"
    if (l ~ /mozilla|opera/) return "browser"
    return "other"
}

function count(kindname, method, status) {
    KIND[kindname]++
    METH[method]++
    STAT[substr(status, 1, 1) "xx"]++
}

# shell sort of V[1..n] (numbers, ascending)
function vsort(n,    gap, i, j, t) {
    for (gap = int(n / 2); gap > 0; gap = int(gap / 2))
        for (i = gap + 1; i <= n; i++) {
            t = V[i]
            for (j = i; j > gap && V[j - gap] > t; j -= gap) V[j] = V[j - gap]
            V[j] = t
        }
}

function pct(n, p,    i) {
    if (n == 0) return 0
    i = int(p * n + 0.999999); if (i < 1) i = 1; if (i > n) i = n
    return V[i]
}

# Top-N by value of array A (index → number), descending; fills TOPK[1..m], returns m
function topn(A, m,    k, i, j, cnt, t) {
    cnt = 0
    for (k in A) {
        if (cnt < m) cnt++
        else if (A[k] <= A[TOPK[cnt]]) continue
        TOPK[cnt] = k
        for (i = cnt; i > 1 && A[TOPK[i]] > A[TOPK[i - 1]]; i--) { t = TOPK[i]; TOPK[i] = TOPK[i - 1]; TOPK[i - 1] = t }
    }
    return cnt
}

function distribution(title, A, unit,    k, n, i, over) {
    n = 0
    for (k in A) V[++n] = A[k]
    vsort(n)
    printf "%s (%d %s)\n", title, n, unit
    if (n == 0) { print "  (none)"; return }
    printf "  p50 %d  p75 %d  p90 %d  p95 %d  p99 %d  p99.9 %d  max %d\n", pct(n, .5), pct(n, .75), pct(n, .9), pct(n, .95), pct(n, .99), pct(n, .999), V[n]
    printf "  over:"
    for (i = 1; i <= nthr; i++) {
        over = 0
        for (k = n; k >= 1 && V[k] > THR[i] + 0; k--) over++
        printf "  >%s: %d", THR[i], over
    }
    printf "\n"
}

# ── parsers ──────────────────────────────────────────────────────────────────

{ line = $0; gsub(/\033\[[0-9;]*m/, "", line); gsub(/\r/, "", line) }

# nginx: ... IP - user [25/Nov/2026:00:35:30 +0100] "GET /x HTTP/1.1" 200 123 "ref" "ua" ...
match(line, /\[[0-9]+\/[A-Za-z]+\/[0-9]+:[0-9]+:[0-9]+:[0-9]+ [-+][0-9]+\] "/) {
    pre = substr(line, 1, RSTART - 1)
    ts = substr(line, RSTART + 1, RLENGTH - 4)
    rest = substr(line, RSTART + RLENGTH - 1)
    nw = split(pre, w, " ")
    if (nw < 3) { bad(); next }
    ip = w[nw - 2]
    if (ip !~ /^[0-9a-fA-F:.]+$/) { bad(); next }
    np = split(rest, q, "\"")
    split(q[2], rq, " "); method = rq[1]; path = rq[2]
    split(q[3], sb, " "); status = sb[1]; bytes = sb[2] + 0
    ref = q[4]; ua = (np >= 6) ? q[6] : ""
    split(ts, t, /[\/: ]/)
    hour = t[3] "-" MON[t[2]] "-" t[1] "T" t[4]
    day = t[3] "-" MON[t[2]] "-" t[1]
    NG++
    if (hour < FIRST || FIRST == "") FIRST = hour
    if (hour > LAST) LAST = hour

    k = kind(path)
    count(k, method, status)
    BYTES[k] += bytes
    c = client(ipkey(ip))
    CREQ[c]++
    CK[c, k]++
    if (substr(status, 1, 1) == "4") C4[c]++
    if (ref != "" && ref != "-") CREF[c]++
    UA_C[c, ua]++
    if (!((c, ua) in CUA_SEEN)) { CUA_SEEN[c, ua] = 1; CUAN[c]++ }
    if (CUAN[c] == 1) CUA1[c] = ua
    u = uaclass(ua)
    UAREQ[u]++
    if (!((u, c) in UACL)) { UACL[u, c] = 1; UACLN[u]++; CCLASS[c] = (c in CCLASS) ? CCLASS[c] "+" u : u }
    if (!((c, hour) in CHSEEN)) { CHSEEN[c, hour] = 1; CHOURS[c]++ }
    HREQ_NG[hour]++
    if (THING != "") {
        if (!((c, hour, THING) in SEEN_H)) { SEEN_H[c, hour, THING] = 1; CH[c, hour]++; if (CH[c, hour] > CMAXH[c]) CMAXH[c] = CH[c, hour] }
        if (!((c, day, THING) in SEEN_D)) { SEEN_D[c, day, THING] = 1; CD[c, day]++; if (CD[c, day] > CMAXD[c]) CMAXD[c] = CD[c, day] }
        if (!((c, THING) in SEEN_A)) { SEEN_A[c, THING] = 1; CTH[c]++ }
        if (substr(THING, 1, 2) == "i:") ITEMS_ALL[substr(THING, 3)] = 1
    }
    next
}

# f0ckm: 2026-11-25T00:35:30.123456Z  INFO 12.34ms  GET  200  /path   (also WARN/ERROR messages)
match(line, /[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9][.0-9]*Z? +(TRACE|DEBUG|INFO|WARN|ERROR) /) {
    ts = substr(line, RSTART, 13)
    rest = substr(line, RSTART + RLENGTH)
    lvl = substr(line, RSTART, RLENGTH); sub(/^[^ ]+ +/, "", lvl); sub(/ +$/, "", lvl)
    nw = split(rest, w, " ")
    if (lvl == "INFO" && nw >= 4 && w[1] ~ /^[0-9.]+ms$/ && w[3] ~ /^[0-9][0-9][0-9]$/) {
        FM++
        hour = ts
        if (hour < FFIRST || FFIRST == "") FFIRST = hour
        if (hour > FLAST) FLAST = hour
        ms = w[1]; sub(/ms$/, "", ms)
        k = kind(w[4])
        FKIND[k]++
        FSTAT[substr(w[3], 1, 1) "xx"]++
        FMS[k] += ms
        if (ms + 0 > FMSMAX[k]) FMSMAX[k] = ms + 0
        FH[hour]++
        if (k == "item") FHI[hour]++
        if (k == "file") FHF[hour]++
        mi = substr(line, RSTART, 16)
        FMIN[mi]++
        next
    }
    # messages: count by [TAG]; the scraper guard's blocks are listed
    msg = rest
    if (match(msg, /\[[A-Z0-9 _-]+\]/)) tag = substr(msg, RSTART, RLENGTH); else tag = "(untagged)"
    MSG[lvl " " tag]++
    if (index(msg, "[SCRAPER] Blocked")) SCRAPER[++nscr] = ts " " substr(mask_ips(msg), 1, 230)
    next
}

# Anubis: JSON lines
/"msg":"/ {
    AN++
    m = line; sub(/.*"msg":"/, "", m); sub(/".*/, "", m)
    ANMSG[m]++
    next
}

{ bad() }

function bad() {
    if (line ~ /^[ \t]*$/) return
    UNP++
    if (UNP <= SAMPLES + 0) UNPS[UNP] = substr(mask_ips(line), 1, 200)
}

# ── report ───────────────────────────────────────────────────────────────────

END {
    print "== log-stats =="
    printf "lines: nginx %d, f0ckm requests %d, anubis %d, unparsed %d\n", NG, FM, AN, UNP
    if (NG) printf "nginx range: %s .. %s (hours, UTC offset as logged)\n", FIRST, LAST
    if (FM) printf "f0ckm range: %s .. %s\n", FFIRST, FLAST
    for (i = 1; i <= UNP && i <= SAMPLES + 0; i++) printf "  unparsed: %s\n", UNPS[i]

    if (NG) {
        print ""
        print "== nginx: requests by kind (count, share, MB out) =="
        for (k in KIND) printf "  %-7s %10d  %5.1f%%  %10.1f\n", k, KIND[k], 100 * KIND[k] / NG, BYTES[k] / 1048576
        printf "  status:"; for (s in STAT) printf "  %s %d", s, STAT[s]; printf "\n"
        printf "  method:"; for (s in METH) printf "  %s %d", s, METH[s]; printf "\n"
        printf "  clients: %d (distinct items seen: %d)\n", ncl, length_of(ITEMS_ALL)

        print ""
        print "== nginx: user-agent classes (requests, clients) =="
        for (u in UAREQ) printf "  %-9s %10d  %7d\n", u, UAREQ[u], UACLN[u]

        print ""
        print "== nginx: things (distinct items + /b/ files) per client =="
        distribution("per client-hour", CH, "client-hours with things")
        distribution("per client-day", CD, "client-days with things")
        distribution("per client, whole log", CTH, "clients with things")
        # browsers only: what people (or scrapers posing as browsers) do
        for (k in CH) { split(k, kk, SUBSEP); if (CCLASS[kk[1]] == "browser") CHB[k] = CH[k] }
        distribution("per client-hour, browser UA only", CHB, "client-hours")

        print ""
        print "== nginx: share of requests per kind, for clients by max things/hour =="
        print "  (people load thumbs and assets; scrapers often fetch only items/files)"
        nb = split("1-49 50-249 250-999 1000+", BN, " ")
        for (c in CREQ) {
            m = CMAXH[c] + 0
            b = (m >= 1000) ? 4 : (m >= 250) ? 3 : (m >= 50) ? 2 : (m >= 1) ? 1 : 0
            if (!b) continue
            BC[b]++; BR[b] += CREQ[c]
            BK[b, "item"] += CK[c, "item"]; BK[b, "file"] += CK[c, "file"]; BK[b, "thumb"] += CK[c, "thumb"]
            BK[b, "asset"] += CK[c, "asset"]; BK[b, "api"] += CK[c, "api"] + CK[c, "ajax"]; BK[b, "page"] += CK[c, "page"]
            BREF[b] += CREF[c]
        }
        printf "  %-9s %7s %9s  %5s %5s %5s %5s %5s %5s %6s\n", "max/h", "clients", "requests", "item", "file", "thumb", "asset", "api", "page", "refer"
        for (b = 1; b <= nb; b++) {
            if (!BR[b]) continue
            printf "  %-9s %7d %9d  %4.0f%% %4.0f%% %4.0f%% %4.0f%% %4.0f%% %4.0f%% %5.0f%%\n", BN[b], BC[b], BR[b],
                100 * BK[b, "item"] / BR[b], 100 * BK[b, "file"] / BR[b], 100 * BK[b, "thumb"] / BR[b],
                100 * BK[b, "asset"] / BR[b], 100 * BK[b, "api"] / BR[b], 100 * BK[b, "page"] / BR[b], 100 * BREF[b] / BR[b]
        }

        print ""
        printf "== nginx: top %d clients by max things in one hour ==\n", TOP
        printf "  %-7s %-16s %6s %6s %6s %7s %6s %6s %6s %6s %5s %4s %5s  %s\n", "client", "net", "max/h", "max/d", "things", "reqs", "items", "files", "thumbs", "assets", "4xx%", "hrs", "refr%", "class | top user agent"
        m = topn(CMAXH, TOP)
        for (i = 1; i <= m; i++) {
            c = TOPK[i]
            printf "  %-7s %-16s %6d %6d %6d %7d %6d %6d %6d %6d %4.0f%% %4d %4.0f%%  %s | %s%s\n", c, CMASK[c], CMAXH[c], CMAXD[c], CTH[c], CREQ[c],
                CK[c, "item"], CK[c, "file"], CK[c, "thumb"], CK[c, "asset"], 100 * C4[c] / CREQ[c], CHOURS[c], 100 * CREF[c] / CREQ[c],
                CCLASS[c], substr(CUA1[c], 1, 90), (CUAN[c] > 1 ? " (+" (CUAN[c] - 1) " UAs)" : "")
        }

        print ""
        printf "== nginx: top %d user agents (requests) ==\n", TOP
        for (k in UA_C) { split(k, kk, SUBSEP); UAT[kk[2]] += UA_C[k] }
        m = topn(UAT, TOP)
        for (i = 1; i <= m; i++) printf "  %8d  %s\n", UAT[TOPK[i]], substr(TOPK[i], 1, 150)

        print ""
        print "== nginx: busiest hours (requests) =="
        m = topn(HREQ_NG, 10)
        for (i = 1; i <= m; i++) printf "  %s  %d\n", TOPK[i], HREQ_NG[TOPK[i]]
    }

    if (FM) {
        print ""
        print "== f0ckm: requests by kind (count, share, avg ms, max ms) =="
        for (k in FKIND) printf "  %-7s %10d  %5.1f%%  %8.1f  %9.1f\n", k, FKIND[k], 100 * FKIND[k] / FM, FMS[k] / FKIND[k], FMSMAX[k]
        printf "  status:"; for (s in FSTAT) printf "  %s %d", s, FSTAT[s]; printf "\n"
        print ""
        print "== f0ckm: per hour, site-wide (all clients together) =="
        n = 0; for (h in FH) V[++n] = FH[h]; vsort(n)
        printf "  requests/hour  p50 %d  p90 %d  max %d  (%d hours)\n", pct(n, .5), pct(n, .9), V[n], n
        n = 0; for (h in FH) V[++n] = FHI[h] + 0; vsort(n)
        printf "  item opens/h   p50 %d  p90 %d  max %d\n", pct(n, .5), pct(n, .9), V[n]
        n = 0; for (h in FH) V[++n] = FHF[h] + 0; vsort(n)
        printf "  file loads/h   p50 %d  p90 %d  max %d\n", pct(n, .5), pct(n, .9), V[n]
        n = 0; for (h in FMIN) V[++n] = FMIN[h]; vsort(n)
        printf "  requests/min   p50 %d  p99 %d  max %d\n", pct(n, .5), pct(n, .99), V[n]
        print "  busiest hours (requests, item opens, file loads):"
        m = topn(FH, 10)
        for (i = 1; i <= m; i++) printf "    %s  %7d %7d %7d\n", TOPK[i], FH[TOPK[i]], FHI[TOPK[i]], FHF[TOPK[i]]
    }

    if (length_of(MSG)) {
        print ""
        print "== f0ckm: log messages by level and [TAG] =="
        m = topn(MSG, 30)
        for (i = 1; i <= m; i++) printf "  %8d  %s\n", MSG[TOPK[i]], TOPK[i]
    }
    if (nscr) {
        print ""
        printf "== f0ckm: scraper guard blocks (%d) ==\n", nscr
        for (i = 1; i <= nscr && i <= 50; i++) print "  " SCRAPER[i]
        if (nscr > 50) printf "  ... %d more\n", nscr - 50
    }
    if (AN) {
        print ""
        print "== anubis: messages =="
        m = topn(ANMSG, 20)
        for (i = 1; i <= m; i++) printf "  %8d  %s\n", ANMSG[TOPK[i]], TOPK[i]
    }
}

function length_of(A,    k, n) { n = 0; for (k in A) n++; return n }
