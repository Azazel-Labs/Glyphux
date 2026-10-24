extends Node3D
## Flies the camera around and towards the labels: their fields follow the distance.

var time := 0.0

@onready var camera: Camera3D = $Camera


func _process(delta: float) -> void:
	time += delta
	var angle := 0.35 * sin(time * 0.3)
	var distance := 4.5 + 2.5 * sin(time * 0.45)
	var target := Vector3(0.0, 1.2, 0.0)
	camera.position = target + Vector3(sin(angle) * distance, 1.0 + 0.3 * distance, cos(angle) * distance)
	camera.look_at(target)
