// The state every Glyphux text shares: the glyph atlas (its Rust side, and its textures),
// the color glyph pages, and the materials. One per engine: the editor's viewports and Play
// In Editor share it.
//
// Rust (the plugin's rust/src/server.rs) owns the atlas' cache. When labels request glyphs it
// has not generated yet, Rust describes one batch through the methods below (the #[cpp_base]
// trait Gpu), and SubmitBatch runs it in the render graph before the frame draws. Color glyphs
// are compiled on the CPU, in Rust, and copied into their page with WriteColorRows.

#pragma once

#include "CoreMinimal.h"
#include "Subsystems/EngineSubsystem.h"
#include "Tickable.h"
#include "GlyphuxSubsystem.generated.h"

class FGlyphuxGpu;
struct FGlyphuxBatch;
class UFontFace;
class UMaterialInterface;
class UTexture2D;

UCLASS()
class GLYPHUX_API UGlyphuxSubsystem : public UEngineSubsystem, public FTickableGameObject
{
	GENERATED_BODY()

public:
	/** Side of a page, and the most pages the atlas and the color glyphs have (glyphux-gpu's AtlasConfig::default()). */
	static constexpr int32 PageSize = 1024;
	static constexpr int32 MaxPages = 6;

	/** The subsystem, once the engine has started it. */
	static UGlyphuxSubsystem* Get();

	virtual void Initialize(FSubsystemCollectionBase& Collection) override;
	virtual void Deinitialize() override;

	// FTickableGameObject: starts each frame of the atlas.
	virtual void Tick(float DeltaTime) override;
	virtual TStatId GetStatId() const override;
	virtual bool IsTickable() const override { return bStarted; }
	virtual bool IsTickableInEditor() const override { return true; }
	virtual bool IsTickableWhenPaused() const override { return true; }

	/** The atlas' fields: MaxPages pages of PageSize texels stacked vertically (RGBA: MTSDF). */
	UTexture2D* GetAtlasTexture() const { return Atlas; }

	/** The color glyph pages, stacked the same way (RGBA: region distances). */
	UTexture2D* GetColorTexture() const { return ColorPages; }

	/** The material of widgets, or of components (with or without depth test); null when it does not load. */
	UMaterialInterface* GetUIMaterial();
	UMaterialInterface* GetWorldMaterial(bool bDepthTest);
	UMaterialInterface* GetGlyphMaterial();
	UMaterialInterface* GetSurfaceMaterial();

	/** The atlas' counters, for debugging. */
	UFUNCTION(BlueprintCallable, Category = "Glyphux")
	static FString GetGlyphuxStats();

	// Called from Rust (the #[cpp_base] trait Gpu in rust/src/lib.rs).
	void BeginBatch(uint32 Pages);
	void BatchInput(uint32 Binding, TConstArrayView<uint8> Bytes);
	void BatchOutput(uint32 Binding, uint32 Size);
	void BatchDispatch(uint32 Kernel, uint32 X, uint32 Y);
	void BatchRows(uint32 Layer, uint32 Start, uint32 End);
	void SubmitBatch();
	void WriteColorRows(uint32 Page, uint32 X, uint32 Y, uint32 Width, uint32 Height, TConstArrayView<uint8> Bytes);
	FString FontPath(UFontFace* Font);
	TArray<uint8> FontBytes(UFontFace* Font);
	UFontFace* LoadFont(const FString& Path);

private:
	UPROPERTY(Transient)
	TObjectPtr<UTexture2D> Atlas;

	UPROPERTY(Transient)
	TObjectPtr<UTexture2D> ColorPages;

	UPROPERTY(Transient)
	TObjectPtr<UMaterialInterface> UIMaterial;

	UPROPERTY(Transient)
	TObjectPtr<UMaterialInterface> WorldMaterials[2];

	UPROPERTY(Transient)
	TObjectPtr<UMaterialInterface> GlyphMaterial;

	UPROPERTY(Transient)
	TObjectPtr<UMaterialInterface> SurfaceMaterial;

	TSharedPtr<FGlyphuxGpu, ESPMode::ThreadSafe> Gpu;
	/** Shared (not unique) so that this header needs no complete FGlyphuxBatch to destroy it. */
	TSharedPtr<FGlyphuxBatch, ESPMode::ThreadSafe> Pending;
	bool bStarted = false;
	bool bWarnedMaterials = false;
};
