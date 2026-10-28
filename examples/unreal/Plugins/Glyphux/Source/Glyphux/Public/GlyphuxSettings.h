// Project Settings > Plugins > Glyphux: the materials the text is drawn with (MATERIALS.md).

#pragma once

#include "CoreMinimal.h"
#include "Engine/DeveloperSettings.h"
#include "GlyphuxSettings.generated.h"

class UMaterialInterface;

UCLASS(Config = Game, DefaultConfig, meta = (DisplayName = "Glyphux"))
class GLYPHUX_API UGlyphuxSettings : public UDeveloperSettings
{
	GENERATED_BODY()

public:
	UGlyphuxSettings();

	virtual FName GetCategoryName() const override { return TEXT("Plugins"); }

	/** The material of Glyphux Text widgets: User Interface domain, Translucent (see the plugin's MATERIALS.md). */
	UPROPERTY(Config, EditAnywhere, Category = "Materials")
	TSoftObjectPtr<UMaterialInterface> UIMaterial;

	/** The material of Glyphux Text components: Surface, Translucent, Unlit, two sided. */
	UPROPERTY(Config, EditAnywhere, Category = "Materials")
	TSoftObjectPtr<UMaterialInterface> WorldMaterial;

	/** The same with Disable Depth Test, for components with No Depth Test. */
	UPROPERTY(Config, EditAnywhere, Category = "Materials")
	TSoftObjectPtr<UMaterialInterface> WorldMaterialNoDepthTest;

	/** One glyph per Niagara sprite or mesh fragment (GlyphuxShadeGlyph): Surface, Translucent, Unlit, two sided. */
	UPROPERTY(Config, EditAnywhere, Category = "Materials")
	TSoftObjectPtr<UMaterialInterface> GlyphMaterial;

	/** A whole text on any mesh, by its texture coordinates (GlyphuxShadeSurface): Surface, Translucent, Unlit, two sided. */
	UPROPERTY(Config, EditAnywhere, Category = "Materials")
	TSoftObjectPtr<UMaterialInterface> SurfaceMaterial;
};
