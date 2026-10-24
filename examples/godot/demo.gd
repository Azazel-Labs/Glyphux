extends Node
## Switches the demo's scenes and takes screenshots (autoload).
##
## Keys: 1 the 2D labels, 2 rich text, 3 text in 3D.
## `godot --path . -- --scene=world --screenshot=out.png` opens a scene, saves the window
## after three seconds and quits.

const SCENES := {"main": "res://main.tscn", "rich": "res://rich.tscn", "world": "res://world.tscn"}

var screenshot := ""
var elapsed := 0.0


func _ready() -> void:
	for arg in OS.get_cmdline_user_args():
		if arg.begins_with("--screenshot="):
			screenshot = arg.trim_prefix("--screenshot=")
		elif arg.begins_with("--scene="):
			var scene: String = SCENES.get(arg.trim_prefix("--scene="), "")
			if scene:
				get_tree().change_scene_to_file.call_deferred(scene)


func _process(delta: float) -> void:
	elapsed += delta
	if OS.get_cmdline_user_args().has("--updates") and Engine.get_process_frames() % 60 == 0:
		print("frame ", Engine.get_process_frames(), ": ", Glyphux.get_stats().get("updates"), " label updates")
	if screenshot and elapsed > 3.0:
		get_viewport().get_texture().get_image().save_png(screenshot)
		print("saved ", screenshot, " (label updates so far: ", Glyphux.get_stats().get("updates"), ")")
		screenshot = ""
		get_tree().quit()


func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventKey and event.pressed and not event.echo:
		var scene: String = {KEY_1: SCENES.main, KEY_2: SCENES.rich, KEY_3: SCENES.world}.get(event.keycode, "")
		if scene:
			get_tree().change_scene_to_file(scene)
