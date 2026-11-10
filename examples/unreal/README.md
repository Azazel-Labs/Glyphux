# Glyphux in Unreal: demo

An Unreal Engine 5.8 project with the [Glyphux plugin](Plugins/Glyphux) and one actor,
**Glyphux Demo** (`Source/GlyphuxDemo`):

* a title, a Glyphux Text component with BBCode, a glow, a shadow and a wave;
* a turning cylinder with a text wrapped around it: a second text component lays it out
  without drawing it, and the cylinder's surface material draws it from the atlas;
* the title breaking into fragments every few seconds that fly apart and reassemble:
  instanced static meshes of the plugin's shards (`DA_GlyphuxShards`), or, when its Shatter
  System is set, Niagara particles (a system built as
  [NIAGARA.md](Plugins/Glyphux/NIAGARA.md) says).

## Run

1. Install [rustup](https://rustup.rs), once: the first build installs the Rust toolchain,
   the target and the crates (Ferreal among them) it needs.
2. `GenerateProjectFiles.bat`, for the Visual Studio solution (optional).
3. `SetupDemo.bat`, once: it builds the `GlyphuxDemoEditor` target (which runs cargo for the
   plugin's Rust), then makes the assets and the level with
   [Content/Python/setup_demo.py](Content/Python/setup_demo.py), without opening the editor.
   Run it again to rebuild them.
4. `OpenEditor.bat`: it opens `L_GlyphuxDemo`. Press Play.

Without the bat files: build the editor target, open the project, and run
`Content/Python/setup_demo.py` (Tools > Execute Python Script).

The project ships no binary assets: the scripts make the materials, the meshes and the
level, the fonts are the engine's, and a Niagara system is yours to build from the recipe.
