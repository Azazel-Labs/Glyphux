# Instructions for coding agents

## Writing

- Never use the em dash (U+2014) or the en dash (U+2013), anywhere: documentation, code
  comments, doc comments, commit messages, UI strings, generated figures. Use a colon, a
  comma, parentheses or a separate sentence instead, whichever reads best; for ranges, write
  "3 to 5".
- Before finishing, check that none slipped in: `git grep -nP "[\x{2013}\x{2014}]"` must
  find nothing.
