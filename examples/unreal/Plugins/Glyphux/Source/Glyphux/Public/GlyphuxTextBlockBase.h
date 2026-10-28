// The C++ parent of UGlyphuxTextBlock, the UMG widget whose properties and text logic are
// written in Rust (rust/src/block.rs). It owns the Slate widget, asks Rust to refresh the text
// when its size on screen changes (RefreshText), and draws what Rust gives it (SetDrawData)
// as Slate custom vertices with the UI material.

#pragma once

#include "CoreMinimal.h"
#include "Components/Widget.h"
#include "Styling/SlateBrush.h"
#include "GlyphuxTextBlockBase.generated.h"

class SGlyphuxText;
class UGlyphuxDrawData;

DECLARE_DYNAMIC_MULTICAST_DELEGATE_OneParam(FOnGlyphuxLinkClicked, const FString&, Link);

UCLASS(Abstract)
class GLYPHUX_API UGlyphuxTextBlockBase : public UWidget
{
	GENERATED_BODY()

public:
	UGlyphuxTextBlockBase(const FObjectInitializer& ObjectInitializer);

	/** A [url] link was clicked: its [url=...] value, or its text. */
	UPROPERTY(BlueprintAssignable, Category = "Glyphux")
	FOnGlyphuxLinkClicked OnLinkClicked;

	// Overridden in Rust.
	/** Lays out and draws the text for Scale screen pixels per Slate unit, wrapped at Width. */
	virtual void RefreshText(float Scale, float Width) {}
	/** The link at Position (Slate units from the widget's top-left); empty where there is none. */
	virtual FString LinkAt(const FVector2D& Position) const { return FString(); }
	/** The Slate widget is gone: the text stops retaining atlas pages. */
	virtual void ReleaseText() {}

	// Called from Rust (the #[cpp_base] trait BlockSink).
	void SetDrawData(TConstArrayView<float> Quads, TConstArrayView<float> Records, TConstArrayView<float> Tables, float PxRange, bool bAnimated);
	void SetContentSize(float Width, float Height);
	void InvalidateText();

	// For the Slate widget.
	/** Refreshes the text if it is dirty or Scale or Width changed since it was drawn. */
	void PrepareToPaint(float Scale, float Width);
	FVector2D GetContentSize() const { return ContentSize; }
	const UGlyphuxDrawData* GetDrawData() const { return Draw; }
	/** The brush whose resource is the material instance (null resource: nothing to draw). */
	const FSlateBrush& GetBrush() const { return Brush; }

protected:
	virtual TSharedRef<SWidget> RebuildWidget() override;
	virtual void ReleaseSlateResources(bool bReleaseChildren) override;
#if WITH_EDITOR
	virtual const FText GetPaletteCategory() override;
#endif

private:
	TSharedPtr<SGlyphuxText> MyText;

	UPROPERTY(Transient)
	TObjectPtr<UGlyphuxDrawData> Draw;

	FSlateBrush Brush;
	FVector2D ContentSize = FVector2D::ZeroVector;
	float DrawnScale = -1.0f;
	float DrawnWidth = -1.0f;
	bool bTextDirty = true;
};
