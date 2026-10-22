Fonts for the demos, all under licenses that allow redistribution:

* `Inter-Regular.otf` — [Inter](https://github.com/rsms/inter) 4.1, SIL Open Font License 1.1
  (`Inter-OFL.txt`). Embedded by the web demo; also the default font of the VS Code tasks and
  launch configurations. `InterVariable-Latin.ttf` is Inter's variable font (weight and
  optical size axes) subset to ASCII, for the title's animated weight.
* `NotoSans{JP,SC,KR,Arabic,Devanagari,Thai}-Subset.ttf` — [Noto Sans](https://fonts.google.com/noto)
  from the Google Fonts repository, SIL Open Font License 1.1 (`NotoSans*-OFL.txt`), fixed at
  the Regular weight and subset with fontTools to the characters the demo shows (plus all
  kana in JP), about 390 KB together. The web demo loads them as fallbacks for the scene's
  CJK, Arabic, Devanagari and Thai text; the native `text_demo` adds every font in the
  folder of the font it is given.
