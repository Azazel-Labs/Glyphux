"""Makes everything the demo needs: the plugin's materials and shards, and the demo's level.

SetupDemo.bat runs it without opening the editor (the pythonscript commandlet); in the
editor, Tools > Execute Python Script, then this file, does the same. Running it again
rebuilds the assets and puts the level back as this script makes it.

It writes:

* /Glyphux/Materials and /Glyphux/Shards: glyphux_materials.py and glyphux_shards.py, the
  plugin's scripts;
* /Game/Maps/L_GlyphuxDemo: the engine's Basic level (floor, sky, sun), a Glyphux Demo
  actor above the floor with its Shards set, and a Player Start facing it. Config/
  DefaultEngine.ini opens it in the editor and in the game.
"""

import os
import runpy

import unreal

MAP = "/Game/Maps/L_GlyphuxDemo"
TEMPLATE = "/Engine/Maps/Templates/Template_Default"
SHARDS = "/Glyphux/Shards/DA_GlyphuxShards"

# The demo above the floor (the cylinder reaches 60 units below its center), and the player
# in front of the text, which faces +X.
DEMO_AT = unreal.Vector(0.0, 0.0, 120.0)
PLAYER_AT = unreal.Vector(650.0, 0.0, 200.0)
PLAYER_FACING = unreal.Rotator(roll=0.0, pitch=0.0, yaw=180.0)

ASSETS = unreal.EditorAssetLibrary


def plugin_assets():
    # Their main() runs on import: run them as scripts, every time.
    for script in ("glyphux_materials", "glyphux_shards"):
        runpy.run_module(script, run_name="__main__")


def level():
    levels = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem)
    # The file, not the asset registry: a commandlet's registry may not list the map yet.
    file = os.path.join(unreal.Paths.project_content_dir(), MAP[len("/Game/"):] + ".umap")
    if os.path.isfile(file):
        ok = levels.load_level(MAP)
    else:
        ok = levels.new_level_from_template(MAP, TEMPLATE)
    if not ok:
        raise RuntimeError("Glyphux: could not open or create " + MAP)

    actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
    # The demo and the start as this script places them: the old ones go.
    for actor in actors.get_all_level_actors():
        if isinstance(actor, (unreal.GlyphuxDemoActor, unreal.PlayerStart)):
            actors.destroy_actor(actor)

    demo = actors.spawn_actor_from_class(unreal.GlyphuxDemoActor, DEMO_AT)
    demo.set_actor_label("Glyphux Demo")
    demo.set_editor_property("shards", ASSETS.load_asset(SHARDS))
    start = actors.spawn_actor_from_class(unreal.PlayerStart, PLAYER_AT, PLAYER_FACING)
    start.set_actor_label("Player Start")

    if not levels.save_current_level():
        raise RuntimeError("Glyphux: could not save " + MAP)
    unreal.log("Glyphux: " + MAP)


def main():
    # A commandlet starts before the asset registry has scanned: the scripts ask it what exists.
    unreal.AssetRegistryHelpers.get_asset_registry().scan_paths_synchronous(["/Glyphux", "/Game"], True)
    plugin_assets()
    level()


main()
