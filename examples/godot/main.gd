extends Node2D
## Animates the demo's labels and shows the atlas filling up.
##
## Keys: A shows or hides an atlas page, Page Up and Page Down pick the page (and 1, 2, 3
## switch scenes, see demo.gd). `--atlas` starts with the atlas view shown,
## `--start=SECONDS` that far into the animation.

const INTER := preload("res://fonts/InterVariable-Latin.ttf")
const LADDER := [12, 18, 26, 36]

var time := 0.0
var page := 0
var curve := Curve2D.new()

@onready var zoom: GlyphuxLabel = $Zoom
@onready var weight: GlyphuxLabel = $Weight
@onready var spline: GlyphuxLabel = $Spline
@onready var stats: GlyphuxLabel = $Stats
@onready var atlas_view: ColorRect = $AtlasView


func _ready() -> void:
	for arg in OS.get_cmdline_user_args():
		if arg == "--atlas":
			atlas_view.visible = true
		elif arg.begins_with("--start="):
			time = float(arg.trim_prefix("--start="))
	# Labels made from code take the same properties as the ones in the scene.
	var y := 0.0
	for size in LADDER:
		y += size * 1.25
		var label := GlyphuxLabel.new()
		label.font = INTER
		label.font_size = size
		label.text = "%d px  Sphinx of black quartz, judge my vow." % size
		label.position = Vector2(0, y)
		$Ladder.add_child(label)
		y += 10.0
	# One cubic segment, its control points moved every frame.
	curve.add_point(Vector2.ZERO)
	curve.add_point(Vector2.ZERO)
	spline.curve = curve


func _process(delta: float) -> void:
	time += delta
	# From 3 to 120 screen pixels per em: the atlas switches field levels on the way, and
	# the edges stay sharp in between.
	var px_per_em := 3.0 * pow(40.0, 0.5 + 0.5 * sin(time * 0.6))
	zoom.scale = Vector2.ONE * px_per_em / zoom.font_size
	zoom.rotation = 0.25 * sin(time * 0.4)
	# The variable font's weight axis, breathing from 100 to 900 (as the web demo's title).
	weight.font_weight = 500.0 + 400.0 * sin(time * 1.3)

	# Text scrolling along a wave, each glyph turned to follow it.
	var x0 := 64.0
	var x1 := 760.0
	var wave_y := 425.0
	var amp := 30.0
	var ctrl := [
		Vector2(x0, wave_y),
		Vector2(x0 + (x1 - x0) / 3.0, wave_y - amp * 2.0 * sin(time * 1.1)),
		Vector2(x0 + (x1 - x0) * 2.0 / 3.0, wave_y + amp * 2.0 * sin(time * 1.4 + 1.0)),
		Vector2(x1, wave_y + amp * 0.5 * cos(time * 0.7)),
	]
	curve.set_point_position(0, ctrl[0])
	curve.set_point_out(0, ctrl[1] - ctrl[0])
	curve.set_point_in(1, ctrl[2] - ctrl[3])
	curve.set_point_position(1, ctrl[3])
	var length := curve.get_baked_length()
	var travel := length + spline.get_text_size().x
	spline.curve_offset = length - fmod(time * 90.0, travel)

	var texture := Glyphux.get_atlas_texture()
	if atlas_view.visible and texture:
		atlas_view.material.set_shader_parameter("atlas", texture)
		atlas_view.material.set_shader_parameter("page", page)

	var s := Glyphux.get_stats()
	stats.text = "%s    %d of %d pages of %d x %d texels, %d color pages    %d glyphs generated, %d raised by the detail probe    A: atlas view" % [
		s.adapter, s.pages, s.max_pages, s.page_size, s.page_size, s.color_pages, s.generated, s.probe_upgrades]


func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventKey and event.pressed and not event.echo:
		var pages: int = max(Glyphux.get_stats().get("pages", 1), 1)
		match event.keycode:
			KEY_A:
				atlas_view.visible = not atlas_view.visible
			KEY_PAGEUP:
				page = (page + 1) % pages
			KEY_PAGEDOWN:
				page = (page + pages - 1) % pages
