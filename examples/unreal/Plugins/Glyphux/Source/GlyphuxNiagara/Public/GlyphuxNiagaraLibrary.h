// Glyphux text as Niagara particles. SetNiagaraGlyphs gives a Niagara system a text
// component's glyphs in its user parameters; the system spawns a particle per entry and
// draws it with the glyph material, which reads the glyph's quad index from the particle's
// dynamic material parameter. NIAGARA.md has the emitter recipes.
//
// User parameters set (create them in the system, with these names and types):
//
// * GlyphuxCount (int): how many entries the arrays hold, the particles to spawn.
// * GlyphuxPositions (Vector array): each particle's position at rest.
// * GlyphuxCenters (Vector array): the center of each particle's glyph (for a fragment, the
//   glyph it belongs to; for a glyph, its position).
// * GlyphuxSizes (Vector 2D array): the glyph's quad's width and height.
// * GlyphuxQuads (float array): the glyph's quad index, for the dynamic material parameter.
// * GlyphuxMeshIndices (int array): with a shard set, the fragment's mesh index; else 0.
// * GlyphuxRotation (Quaternion): the text's rotation (the glyphs face its +X).
// * GlyphuxMaterial (Material): the glyph material instance bound to the text, for the
//   renderer's material binding.

#pragma once

#include "CoreMinimal.h"
#include "Kismet/BlueprintFunctionLibrary.h"
#include "GlyphuxNiagaraLibrary.generated.h"

class UGlyphuxShardSet;
class UGlyphuxTextComponentBase;
class UMaterialInterface;
class UNiagaraComponent;

UCLASS()
class GLYPHUXNIAGARA_API UGlyphuxNiagaraLibrary : public UBlueprintFunctionLibrary
{
	GENERATED_BODY()

public:
	/**
	 * Sets System's Glyphux user parameters from Text's glyphs: one entry per glyph, or with
	 * Shards one per fragment of each glyph. Material is the glyph material's base (null: the
	 * project settings'). With bLocalSpace, positions and the rotation are relative to System
	 * (for local space emitters); else world space. Returns the entry count; 0 before Text is
	 * laid out (bind to its On Text Updated and call again).
	 */
	UFUNCTION(BlueprintCallable, Category = "Glyphux", meta = (AdvancedDisplay = "Material,bLocalSpace"))
	static int32 SetNiagaraGlyphs(UNiagaraComponent* System, UGlyphuxTextComponentBase* Text, UGlyphuxShardSet* Shards = nullptr,
		UMaterialInterface* Material = nullptr, bool bLocalSpace = false);
};
