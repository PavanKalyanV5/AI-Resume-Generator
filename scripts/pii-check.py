#!/usr/bin/env python3
"""Block commits whose staged content contains any literal from the privacy-proxy rules.
Prints file names and counts only, never the matched values."""
import json, os, subprocess, sys
p = os.path.expanduser("~/.claude/redaction/redact-rules.json")
if not os.path.exists(p):
    sys.exit("pii-check: rules file missing, refusing to commit")
lits = [(x if isinstance(x, str) else x["value"]).lower() for x in json.load(open(p))["literals"]]
bad = {}
for f in subprocess.run(["git", "diff", "--cached", "--name-only", "--diff-filter=ACM"], capture_output=True, text=True).stdout.split():
    t = subprocess.run(["git", "show", ":" + f], capture_output=True, text=True, errors="ignore").stdout.lower()
    n = sum(l in t for l in lits)
    if n:
        bad[f] = n
if bad:
    sys.exit(f"pii-check: BLOCKED, PII literals in staged files: {bad}")
