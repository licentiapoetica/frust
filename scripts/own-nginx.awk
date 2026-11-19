# Points the nginx server blocks of one domain at f0ckm or at the Anubis bot protection in front of it.
# install.sh runs it over every file of the host's nginx config (nginx -T) and shows the changes first.
#
#   awk -v dom=f0ck.example -v port=8923 -v css=/path/anubis.css -v img=/path/img -v info=/tmp/info \
#       -f scripts/own-nginx.awk site.conf > site.conf.new
#
# In every server {} whose server_name has `dom`, proxy_pass to 127.0.0.1/localhost/[::1] on :1337 or
# :8923 is set to `port`. With 8923 (Anubis) it also adds
#   - the check page's look (locations between the "f0ckm anubis" marker comments; removed again with 1337),
#   - proxy_set_header X-Real-IP where it's missing (Anubis needs it), next to the location's other headers,
#   - with -v subf=1 (nginx has the sub module): new URLs for those files in the check page, so browsers
#     that cached Anubis' own (it lets them keep them a year) fetch ours; that needs Accept-Encoding ""
#     next to the location's headers, so Anubis sends the page uncompressed. As nginx-proxy-look.conf.
# Lines appended to `info`: "names <server names>" per matching block, "backend" per matching block that
# proxies to f0ckm/Anubis directly (none: it goes through an upstream {} or another port), "noxrealip"
# when X-Real-IP is missing and couldn't be added safely.

function code(s) { sub(/#.*/, "", s); return s }
function count(s, ch,   t) { t = code(s); return gsub(ch, "", t) }
function backend(s) {
    return code(s) ~ /proxy_pass[ \t]+https?:\/\/(127\.0\.0\.1|localhost|\[::1\]):(1337|8923)([^0-9]|$)/
}
function indent(s) { sub(/[^ \t].*/, "", s); return s }

function flush(   i, j, k, m, w, c, s, inc, hit, names, hasb, hasx, hasxri, skip, ctx, own, firstloc, ind) {
    hit = 0; names = ""
    for (i = 1; i <= n; i++) {
        c = code(b[i])
        if (c ~ /^[ \t]*server_name[ \t]/) {
            s = c; sub(/^[ \t]*server_name[ \t]+/, "", s); sub(/;.*/, "", s)
            k = split(s, w, /[ \t]+/)
            for (j = 1; j <= k; j++) if (w[j] != "") { names = names " " w[j]; if (tolower(w[j]) == tolower(dom)) hit = 1 }
        }
    }
    if (!hit) { for (i = 1; i <= n; i++) print b[i]; return }
    if (info != "") print "names" names >> info

    # What the block has outside the install.sh markers
    hasb = 0; hasx = 0; hasxri = 0; skip = 0
    for (i = 1; i <= n; i++) {
        if (b[i] ~ /# >>> f0ckm anubis/) { skip = 1; continue }
        if (skip) { if (b[i] ~ /# <<< f0ckm anubis/) skip = 0; continue }
        c = tolower(code(b[i]))
        if (backend(b[i])) hasb = 1
        if (c ~ /xess\.min\.css/) hasx = 1
        if (c ~ /x-real-ip/) hasxri = 1
    }
    if (!hasb) { for (i = 1; i <= n; i++) print b[i]; return }
    if (info != "") print "backend" >> info

    skip = 0; firstloc = 0
    for (i = 1; i <= n; i++) {
        if (b[i] ~ /# >>> f0ckm anubis/) { skip = 1; continue }
        if (skip) { if (b[i] ~ /# <<< f0ckm anubis/) skip = 0; continue }
        line = b[i]
        if (port == 8923 && !hasx && !firstloc && ds[i] == sd + 1 && code(line) ~ /^[ \t]*location[ \t]/) {
            firstloc = 1; ind = indent(line)
            print ind "# >>> f0ckm anubis: the check page's look (install.sh adds and removes this)"
            print ind "location = /.within.website/x/xess/xess.min.css { alias " css "; }"
            print ind "location = /.within.website/x/cmd/anubis/static/img/pensive.webp { alias " img "/checking.webp; }"
            print ind "location = /.within.website/x/cmd/anubis/static/img/happy.webp   { alias " img "/passed.webp; }"
            print ind "location = /.within.website/x/cmd/anubis/static/img/reject.webp  { alias " img "/denied.webp; }"
            if (subf) {
                print ind "sub_filter_once off;"
                print ind "sub_filter 'cachebuster=' 'cachebuster=f0ckm1-';"
                print ind "sub_filter '<script id=\"anubis_version\" type=\"application/json\">\"' '<script id=\"anubis_version\" type=\"application/json\">\"f0ckm1-';"
            }
            print ind "# <<< f0ckm anubis"
        }
        if (backend(line)) sub(/:(1337|8923)/, ":" port, line)
        print line
        if (port == 8923 && backend(b[i])) {
            # A location with its own proxy_set_header lines doesn't inherit the server's, so headers
            # can go right there; without any, adding one would drop the inherited ones. An include
            # (Debian's proxy_params) brings its own, and may already set X-Real-IP.
            own = 0; inc = 0; ctx = ds[i]
            for (m = i - 1; m >= 1 && ds[m] >= ctx; m--) if (ds[m] == ctx) { c = code(b[m]); if (c ~ /proxy_set_header/) own = 1; if (c ~ /^[ \t]*include[ \t]/) inc = 1 }
            for (m = i + 1; m <= n && ds[m] >= ctx; m++) if (ds[m] == ctx) { c = code(b[m]); if (c ~ /proxy_set_header/) own = 1; if (c ~ /^[ \t]*include[ \t]/) inc = 1 }
            if (!hasxri && !inc) {
                if (own) print indent(b[i]) "proxy_set_header X-Real-IP $remote_addr;"
                else if (info != "") print "noxrealip" >> info
            }
            if (subf && (own || inc)) {
                print indent(b[i]) "# >>> f0ckm anubis: the check page uncompressed, for the sub_filter above"
                print indent(b[i]) "proxy_set_header Accept-Encoding \"\";"
                print indent(b[i]) "# <<< f0ckm anubis"
            }
        }
    }
}

{
    if (!inb && code($0) ~ /^[ \t]*server[ \t]*(\{|$)/) { inb = 1; sd = depth; n = 0; opened = 0 }
    start = depth
    depth += count($0, "\\{") - count($0, "\\}")
    if (!inb) { print; next }
    n++; b[n] = $0; ds[n] = start
    if (depth > sd) opened = 1
    if (opened && depth <= sd) { flush(); inb = 0 }
}

END { if (inb) for (i = 1; i <= n; i++) print b[i] }
