# Generate a deps reference the agent can grep
for crate in $(cargo metadata --format-version 1 | jq -r '.packages[].name'); do
  echo "=== $crate ===" >> .claude/deps-index.txt
  ./extract-exports.sh "$crate" >> .claude/deps-index.txt 2>/dev/null || true
done