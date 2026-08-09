#!/bin/bash
# Discord digest — 12h summary cho channel "ai-lười-chat-tổng-🫂"
# Cú Đêm AI guild | channel ID 1401064189652500490
CHANNEL=1401064189652500490
PROJECT=/Users/tranquangdang21/Projects/discord_cli
DISCORD="$HOME/.cargo/bin/discord"
OUT="$HOME/.discord-digest"
mkdir -p "$OUT"

cd "$PROJECT" || { echo "❌ Cannot cd to $PROJECT"; exit 1; }

HOUR=$(date +%Y-%m-%d_%H)
STAMP=$(date +%Y-%m-%d_%H%M)
RAW="$OUT/$STAMP.txt"

# Trùng giờ thì skip — tránh báo cáo trùng
if ls "$OUT"/${HOUR}*.txt >/dev/null 2>&1; then
  echo "⏭️ Đã digest lúc $HOUR — skip (tránh trùng)."
  exit 0
fi

# Snowflake 12h trước
BEFORE=$(python3 -c "
import time
ts = int((time.time() - 12*3600) * 1000) - 1
print(ts * 2**22 + 1)
")

# Fetch transcript compact
"$DISCORD" read "$CHANNEL" -l 1000 --before "$BEFORE" --transcript > "$RAW" 2>/tmp/digest_err.txt
RC=$?
if [ $RC -ne 0 ]; then
  echo "❌ Digest failed (exit $RC): $(cat /tmp/digest_err.txt)"
  echo "$(date +%FT%T) FAIL $RC" >> "$OUT/digest.log"
  exit $RC
fi

MSGCOUNT=$(grep -cE '^\[' "$RAW" 2>/dev/null || echo 0)
if [ "$MSGCOUNT" -eq 0 ]; then
  echo "📭 Không có tin nhắn trong 12h qua. (file: $RAW)"
  echo "$(date +%FT%T) OK 0 msgs" >> "$OUT/digest.log"
  exit 0
fi

# Trích link kèm ngữ cảnh → mục ## LINKS
python3 - "$RAW" <<'PYEOF'
import re, sys
path = sys.argv[1]
try:
    text = open(path, encoding='utf-8', errors='replace').read()
except Exception as e:
    print(f"❌ Read error: {e}")
    sys.exit(1)

lines = text.splitlines()
url_re = re.compile(r'https?://[^\s)\]}>"\'，。]+')
found = []
for ln in lines:
    m = re.match(r'\[(\d{2}:\d{2}:\d{2})\] ([^:]+): (.*)', ln)
    if not m:
        continue
    t, author, content = m.group(1), m.group(2).strip(), m.group(3).strip()
    for u in url_re.findall(content):
        found.append((u, author, content[:200], t))

with open(path, 'a', encoding='utf-8') as f:
    f.write("\n## LINKS\n")
    if not found:
        f.write("_Không có link nào trong 12h qua._\n")
    else:
        for u, a, c, t in found:
            f.write(f"- {u} | {a} | {c} | {t}\n")
PYEOF

# Báo cáo ra stdout (AI đọc)
echo "📊 Discord digest — $(date '+%d/%m %H:%M') | channel: ai-lười-chat-tổng-🫂"
echo "Số tin nhắn: $MSGCOUNT | Raw: $RAW"
echo ""
cat "$RAW"
echo ""
echo "$(date +%FT%T) OK $MSGCOUNT msgs -> $RAW" >> "$OUT/digest.log"

# Push raw transcript lên repo GitHub private
REPO_DIR="$HOME/Projects/discord-digests"
if [ -d "$REPO_DIR/.git" ]; then
    mkdir -p "$REPO_DIR/raw"
    cp "$RAW" "$REPO_DIR/raw/"
    cd "$REPO_DIR" || exit 0
    git add -A 2>/dev/null
    git -c user.name="quangdang46" -c user.email="quangdang46@users.noreply.github.com" \
        commit -m "digest: $STAMP ($MSGCOUNT msgs)" 2>/dev/null && \
        git push origin main 2>/dev/null && \
        echo "📤 Pushed: https://github.com/quangdang46/discord-digests/blob/main/raw/$(basename "$RAW")"
fi
