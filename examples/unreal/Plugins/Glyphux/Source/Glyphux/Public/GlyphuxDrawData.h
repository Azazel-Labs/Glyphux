// What one Glyphux text draws, as Rust laid it out (rust/src/draw.rs): its quads, and the
// glyph records and color glyph tables its material reads, in two float textures. The widget
// and the component each keep one.

#pragma once

#include "CoreMinimal.h"
#include "UObject/Object.h"
#include "GlyphuxDrawData.generated.h"

class UMaterialInstanceDynamic;
class UMaterialInterface;
class UTexture2D;

/** A glyph's quad on surfaces and in particles (layout units, y down): a parallelogram. */
struct FGlyphuxSurfaceQuad
{
	FVector2f TopLeft;
	FVector2f TopRight;
	FVector2f BottomLeft;
};

UCLASS(Transient)
class GLYPHUX_API UGlyphuxDrawData : public UObject
{
	GENERATED_BODY()

public:
	/** Replaces what is drawn (the arguments of the Rust's set_draw_data). */
	void Update(TConstArrayView<float> InQuads, TConstArrayView<float> InRecords, TConstArrayView<float> InTables, float InPxRange, bool bInAnimated);

	/**
	 * The instance of Base drawing this text (one per base material, kept); null without Base.
	 * With bSurface, the surface textures are built and bound too (glyph and surface materials).
	 */
	UMaterialInstanceDynamic* GetMaterial(UMaterialInterface* Base, bool bSurface = false);

	/**
	 * Sets this text's parameters on Target, a material of the user's (with the glyph or
	 * surface material function), now and after every update, while Target lives.
	 */
	void Bind(UMaterialInstanceDynamic* Target);

	/** The text box (layout units: left, top, width, height), the GlyphuxTextBox parameter. */
	void SetBox(const FVector4f& InBox);
	const FVector4f& GetBox() const { return Box; }

	/** The quads: GlyphuxRecords::QuadFloats floats each. */
	const TArray<float>& GetQuads() const { return Quads; }
	int32 NumQuads() const;
	bool IsAnimated() const { return bAnimated; }

	/** How far quad Index moves at Time (seconds): its wave and shake, in layout units, y down. */
	FVector2f Animation(int32 Index, float Time) const;

	/**
	 * The textures that draw the text on any surface (GlyphuxShadeSurface in
	 * GlyphuxMaterial.ush), built on first use after each update: the glyph quads, and a grid
	 * over the text listing the quads that touch each cell.
	 */
	UTexture2D* GetSurfaceQuads();
	UTexture2D* GetSurfaceGrid();

	/** The surface quads, in the order of the quads texture (their indices there). */
	const TArray<FGlyphuxSurfaceQuad>& GetSurfaceQuadList();

	UTexture2D* GetRecords() const { return Records; }
	UTexture2D* GetTables() const { return Tables; }
	float GetPxRange() const { return PxRange; }

private:
	void BindTextures();
	void BindTo(UMaterialInstanceDynamic* Target) const;
	void BuildSurface();
	void BuildDrawQuads();

	UPROPERTY()
	TObjectPtr<UTexture2D> Records;

	UPROPERTY()
	TObjectPtr<UTexture2D> Tables;

	UPROPERTY()
	TArray<TObjectPtr<UMaterialInstanceDynamic>> Materials;

	TArray<TWeakObjectPtr<UMaterialInstanceDynamic>> Bound;

	/** Every quad, for the world material (see BuildDrawQuads). */
	UPROPERTY()
	TObjectPtr<UTexture2D> DrawQuads;

	UPROPERTY()
	TObjectPtr<UTexture2D> SurfaceQuads;

	UPROPERTY()
	TObjectPtr<UTexture2D> SurfaceGrid;

	bool bSurfaceBuilt = false;
	bool bWantsSurface = false;
	TArray<FGlyphuxSurfaceQuad> SurfaceList;
	FVector4f Box = FVector4f(0.0f, 0.0f, 1.0f, 1.0f);

	TArray<float> Quads;
	float PxRange = 4.0f;
	bool bAnimated = false;
};
