# Glyphux in Unreal: demo

An Unreal Engine 5.8 project with the [Glyphux plugin](Plugins/Glyphux) and one actor,
**Glyphux Demo** (`Source/GlyphuxDemo`):

* a title, a Glyphux Text component with BBCode, a glow, a shadow and a wave;
* a turning cylinder with a text wrapped around it: a second text component lays it out
  without drawing it, and the cylinder's surface material draws it from the atlas;
* the title breaking into Niagara particles every few seconds, when its Shatter System is
  set (a system built as [NIAGARA.md](Plugins/Glyphux/NIAGARA.md) says) and, for
  fragments rather than whole glyphs, its Shards (`DA_GlyphuxShards`).

## Run

1. Install [Rust](https://rustup.rs). Cargo fetches [Ferreal](https://github.com/Azazel-Labs/Ferreal) on the first
   build.
2. Generate the project files (right-click `GlyphuxDemo.uproject` > Generate Visual Studio
   project files, or `GenerateProjectFiles` with the project) and build the
   `GlyphuxDemoEditor` target, or open the .uproject and let Unreal build it. The build
   runs cargo for the plugin's Rust.
3. In the editor, run `Plugins/Glyphux/Content/Python/glyphux_materials.py` and
   `glyphux_shards.py` (Tools > Execute Python Script).
4. Make a level (File > New Level > Basic), place a **Glyphux Demo** actor, and press Play.

The project ships no binary assets: the plugin's scripts make its materials and meshes, the
fonts are the engine's, and the Niagara system is yours to build from the recipe.
