// How glyphs break: a set of flat fragment meshes that together tile the unit square, each
// textured by where it was in the square. A glyph's fragments are one particle each, all
// drawing the same glyph (the glyph material), each its own part of it: no render targets,
// every fragment samples the atlas like the text does.
//
// Content/Python/glyphux_shards.py makes one (/Glyphux/Shards/DA_GlyphuxShards and its
// meshes); a set of your own needs, per mesh:
//
// * its pivot at the fragment's center, and Center that point in the square;
// * the square in the mesh's YZ plane, facing +X, one unit across: layout x (the square's
//   u) along -Y, layout y (v) along -Z, as the text component lays text out;
// * texture coordinates 0 its position in the square, (0, 0) top-left to (1, 1)
//   bottom-right, on every face that shows the glyph.

#pragma once

#include "CoreMinimal.h"
#include "Engine/DataAsset.h"
#include "GlyphuxShardSet.generated.h"

class UStaticMesh;

USTRUCT(BlueprintType)
struct GLYPHUXNIAGARA_API FGlyphuxShard
{
	GENERATED_BODY()

	/** The fragment, pivot at its center. */
	UPROPERTY(EditAnywhere, BlueprintReadOnly, Category = "Glyphux")
	TObjectPtr<UStaticMesh> Mesh;

	/** Its center in the square: (0, 0) top-left, (1, 1) bottom-right. */
	UPROPERTY(EditAnywhere, BlueprintReadOnly, Category = "Glyphux")
	FVector2D Center = FVector2D(0.5, 0.5);
};

UCLASS(BlueprintType)
class GLYPHUXNIAGARA_API UGlyphuxShardSet : public UPrimaryDataAsset
{
	GENERATED_BODY()

public:
	/**
	 * The fragments, in the order of the Niagara mesh renderer's meshes: a particle's mesh
	 * index (GlyphuxMeshIndices) is its fragment's index here.
	 */
	UPROPERTY(EditAnywhere, BlueprintReadOnly, Category = "Glyphux")
	TArray<FGlyphuxShard> Shards;
};
