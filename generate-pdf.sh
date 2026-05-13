#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

FORCE=false
[[ "${1:-}" == "--force" ]] && FORCE=true

PDF_OPTIONS='{"format":"Letter","printBackground":true,"margin":{"top":"0","bottom":"0","left":"0","right":"0"}}'

MD="COMPLETE_BOOK.md"
PDF="COMPLETE_BOOK.pdf"
CSS="book-style.css"

if [[ ! -f "$MD" ]]; then
  echo "❌ Source not found: $MD"
  exit 1
fi

if [[ ! -f "$CSS" ]]; then
  echo "❌ Stylesheet not found: $CSS"
  exit 1
fi

if [[ "$FORCE" == false && -f "$PDF" && "$PDF" -nt "$MD" && "$PDF" -nt "$CSS" ]]; then
  echo "⏭️  $PDF is up to date (use --force to regenerate)"
  exit 0
fi

echo "📄 Generating $PDF from $MD ..."
# Use local install if available (avoids puppeteer version mismatch with npx cache)
if [[ -x "node_modules/.bin/md-to-pdf" ]]; then
  node_modules/.bin/md-to-pdf "$MD" \
    --stylesheet "$CSS" \
    --highlight-style github-dark \
    --pdf-options "$PDF_OPTIONS"
else
  npx --yes md-to-pdf "$MD" \
    --stylesheet "$CSS" \
    --highlight-style github-dark \
    --pdf-options "$PDF_OPTIONS"
fi

echo "✅ $PDF generated ($(du -h "$PDF" | cut -f1 | xargs))"
