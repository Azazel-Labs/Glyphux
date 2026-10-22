# Instructions for coding agents

## Writing

- Never use the em dash (U+2014), anywhere: documentation, code comments, doc comments,
  commit messages, UI strings, generated figures. Use a colon, a comma, parentheses or a
  separate sentence instead, whichever reads best. En dashes in ranges ("3.2–3.9×", "0–7")
  are fine.
- Before finishing, check that none slipped in: `git grep -nP "\x{2014}"` must find nothing.
