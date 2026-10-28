#include "GlyphuxSettings.h"

#include "Materials/MaterialInterface.h"

UGlyphuxSettings::UGlyphuxSettings()
{
	// Where Content/Python/glyphux_materials.py creates them.
	UIMaterial = TSoftObjectPtr<UMaterialInterface>(FSoftObjectPath(TEXT("/Glyphux/Materials/M_GlyphuxUI.M_GlyphuxUI")));
	WorldMaterial = TSoftObjectPtr<UMaterialInterface>(FSoftObjectPath(TEXT("/Glyphux/Materials/M_GlyphuxWorld.M_GlyphuxWorld")));
	WorldMaterialNoDepthTest = TSoftObjectPtr<UMaterialInterface>(FSoftObjectPath(TEXT("/Glyphux/Materials/M_GlyphuxWorldNoDepthTest.M_GlyphuxWorldNoDepthTest")));
	GlyphMaterial = TSoftObjectPtr<UMaterialInterface>(FSoftObjectPath(TEXT("/Glyphux/Materials/M_GlyphuxGlyph.M_GlyphuxGlyph")));
	SurfaceMaterial = TSoftObjectPtr<UMaterialInterface>(FSoftObjectPath(TEXT("/Glyphux/Materials/M_GlyphuxSurface.M_GlyphuxSurface")));
}
