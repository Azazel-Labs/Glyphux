// The C++ parent of UGlyphuxTextComponent, the scene component whose properties and text
// logic are written in Rust (rust/src/component.rs). It is a procedural mesh: the quads Rust
// gives it (SetDrawData) become a section drawn with the world material, in the component's
// YZ plane facing +X, PixelSize world units per layout pixel.
//
// Every frame it measures how many screen pixels a layout pixel covers at its origin and
// calls RefreshText, which does nothing in Rust unless the text changed or that measure
// needs another atlas field level.

#pragma once

#include "CoreMinimal.h"
#include "ProceduralMeshComponent.h"
#include "GlyphuxTextComponentBase.generated.h"

class UGlyphuxDrawData;
class UMaterialInstanceDynamic;
class UMaterialInterface;

/** One glyph of a text component, placed in the world: for particles, or anything spawned per glyph. */
USTRUCT(BlueprintType)
struct GLYPHUX_API FGlyphuxGlyph
{
	GENERATED_BODY()

	/** The center of the glyph's quad (its glow and shadow included), world space. */
	UPROPERTY(BlueprintReadOnly, Category = "Glyphux")
	FVector Position = FVector::ZeroVector;

	/** The quad's width and height, world units. */
	UPROPERTY(BlueprintReadOnly, Category = "Glyphux")
	FVector2D Size = FVector2D::ZeroVector;

	/** The text's rotation: the glyph faces its +X, its up is +Z. */
	UPROPERTY(BlueprintReadOnly, Category = "Glyphux")
	FQuat Rotation = FQuat::Identity;

	/** The glyph's index in the text's surface quads: what the glyph material draws (GlyphuxShadeGlyph's Quad). */
	UPROPERTY(BlueprintReadOnly, Category = "Glyphux")
	int32 Quad = 0;
};

DECLARE_DYNAMIC_MULTICAST_DELEGATE(FGlyphuxTextUpdated);

UCLASS(Abstract)
class GLYPHUX_API UGlyphuxTextComponentBase : public UProceduralMeshComponent
{
	GENERATED_BODY()

public:
	UGlyphuxTextComponentBase(const FObjectInitializer& ObjectInitializer);

	/** World units per layout pixel. */
	UPROPERTY(EditAnywhere, BlueprintReadOnly, Category = "Glyphux", meta = (ClampMin = "0.0001"))
	float PixelSize = 1.0f;

	/** Faces the camera while playing (turning the component). */
	UPROPERTY(EditAnywhere, BlueprintReadOnly, Category = "Glyphux")
	bool bBillboard = false;

	/** Shows through closer geometry. */
	UPROPERTY(EditAnywhere, BlueprintReadOnly, Category = "Glyphux")
	bool bNoDepthTest = false;

	/**
	 * Draws the text itself. Off for a text that is only the source of particles or of a
	 * surface material: it is still laid out, every frame it is visible.
	 */
	UPROPERTY(EditAnywhere, BlueprintReadOnly, Category = "Glyphux")
	bool bDrawText = true;

	/**
	 * Screen pixels per layout pixel the text's fields are made for; 0 measures it from the
	 * nearest view every frame. Set it for text drawn elsewhere (particles, surfaces), whose
	 * size on screen the component cannot know: 1 is the font size in pixels, 2 twice it.
	 */
	UPROPERTY(EditAnywhere, BlueprintReadOnly, Category = "Glyphux", meta = (ClampMin = "0"))
	float FixedScale = 0.0f;

	/** Broadcast when the text is drawn again (laid out anew): its glyphs may have changed. */
	UPROPERTY(BlueprintAssignable, Category = "Glyphux")
	FGlyphuxTextUpdated OnTextUpdated;

	UFUNCTION(BlueprintCallable, Category = "Glyphux")
	void SetPixelSize(float InPixelSize);

	UFUNCTION(BlueprintCallable, Category = "Glyphux")
	void SetNoDepthTest(bool bInNoDepthTest);

	UFUNCTION(BlueprintCallable, Category = "Glyphux")
	void SetDrawText(bool bInDrawText);

	/** The glyphs as they are now (one per glyph, color glyphs included); empty before the text is laid out. */
	UFUNCTION(BlueprintCallable, Category = "Glyphux")
	TArray<FGlyphuxGlyph> GetGlyphs();

	/**
	 * An instance of Base (null: the glyph material of the project settings) drawing this
	 * text's glyphs one at a time: for Niagara sprites and mesh fragments.
	 */
	UFUNCTION(BlueprintCallable, Category = "Glyphux", meta = (AdvancedDisplay = "Base"))
	UMaterialInstanceDynamic* GetGlyphMaterial(UMaterialInterface* Base = nullptr);

	/**
	 * An instance of Base (null: the surface material of the project settings) drawing this
	 * whole text on any mesh, by the mesh's texture coordinates.
	 */
	UFUNCTION(BlueprintCallable, Category = "Glyphux", meta = (AdvancedDisplay = "Base"))
	UMaterialInstanceDynamic* GetSurfaceMaterial(UMaterialInterface* Base = nullptr);

	/**
	 * Sets this text's parameters on Target (a material of yours with the glyph or surface
	 * material function), now and whenever the text changes.
	 */
	UFUNCTION(BlueprintCallable, Category = "Glyphux")
	void BindToMaterial(UMaterialInstanceDynamic* Target);

	/** What the text draws; null before it is laid out. */
	UGlyphuxDrawData* GetDrawData() const { return Draw; }

	// Overridden in Rust.
	/** Lays out and draws the text for Scale screen pixels per layout pixel. */
	virtual void RefreshText(float Scale) {}
	/** The text stops retaining atlas pages (hidden, unregistered). */
	virtual void ReleaseText() {}

	// Called from Rust (the #[cpp_base] trait ComponentSink).
	void SetDrawData(TConstArrayView<float> Quads, TConstArrayView<float> Records, TConstArrayView<float> Tables, float PxRange, bool bAnimated);
	void SetContentBox(float X, float Y, float Width, float Height);
	void InvalidateText();

	virtual void TickComponent(float DeltaTime, ELevelTick TickType, FActorComponentTickFunction* ThisTickFunction) override;

protected:
	virtual void OnRegister() override;
	virtual void OnUnregister() override;
#if WITH_EDITOR
	virtual void PostEditChangeProperty(FPropertyChangedEvent& PropertyChangedEvent) override;
#endif

private:
	/** Screen pixels per layout pixel at the origin, from the nearest view. */
	float MeasureScale() const;
	/** Rebuilds the mesh section from the quads, moved as animated at Time. */
	void BuildMesh(float Time, bool bPositionsOnly);
	void FaceCamera();
	UGlyphuxDrawData* EnsureDraw();

	UPROPERTY(Transient)
	TObjectPtr<UGlyphuxDrawData> Draw;

	/** The text box (layout pixels): top-left corner and size. */
	FVector4f Box = FVector4f(0.0f, 0.0f, 0.0f, 0.0f);
};
