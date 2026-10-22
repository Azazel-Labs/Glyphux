# Instructions for coding agents

## Writing

- Never use the em dash (—), anywhere: documentation, code comments, doc comments, commit
  messages, UI strings, generated figures. Use a colon, a comma, parentheses or a separate
  sentence instead, whichever reads best. En dashes in ranges ("3.2–3.9×", "0–7") are fine.
- Before finishing, check that none slipped in: `git grep -n "—"` must find nothing.
