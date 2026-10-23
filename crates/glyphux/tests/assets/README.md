# Color glyph test assets

These subsets come from the user's `Gaussian-Color-Prototype-v0.16-Source` bundle.
Only glyph selection and dependent tables were subsetted; the artwork was not redrawn.

- `Noto-COLRv1-Subset.ttf`: Noto Color Emoji v2.047, fire U+1F525, grinning face U+1F600
  and robot U+1F916. The SIL Open Font License is in `Noto-LICENSE.txt`.
- `Twemoji-Subset.ttf`: Twemoji Mozilla, robot U+1F916. Font code is by Mozilla;
  emoji artwork is by the Twemoji project, under CC BY 4.0. See
  `Twemoji-LICENSE.md` and https://github.com/mozilla/twemoji-colr.
  The copied license summary's long dash punctuation was changed to colons to follow
  repository style. License links and terms are otherwise retained.
- `*-prototype-32.rgba32f`: 32 by 32 row-major premultiplied linear RGBA, little-endian
  float32, decoded from the bundle's saved GF16 packets. These test the new compiler
  against the original decoder, including independent rasterization and distance
  processing. They are not an independent font-engine conformance reference.
  `prototype-reference.json` records source-packet and output hashes.

Recreate the subsets with FontTools:

```text
python -m fontTools.subset <bundle>/fonts/Noto-COLRv1.ttf --unicodes=U+1F525,U+1F600,U+1F916 --output-file=crates/glyphux/tests/assets/Noto-COLRv1-Subset.ttf
python -m fontTools.subset <bundle>/fonts/Twemoji.Mozilla.ttf --unicodes=U+1F916 --output-file=crates/glyphux/tests/assets/Twemoji-Subset.ttf
python scripts/color-prototype-reference.py <bundle>
```
