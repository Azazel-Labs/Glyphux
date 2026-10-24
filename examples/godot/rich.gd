extends Control
## Shows which link of the rich text was clicked.


func _ready() -> void:
	$Text.meta_clicked.connect(func(meta): $Clicked.text = "Clicked: %s" % meta)
