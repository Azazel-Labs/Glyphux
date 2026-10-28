// The Slate widget of a Glyphux Text widget: it paints the quads its owner holds as custom
// vertices of the UI material, and finds links under the mouse.

#pragma once

#include "CoreMinimal.h"
#include "Widgets/SLeafWidget.h"

class UGlyphuxTextBlockBase;

class SGlyphuxText : public SLeafWidget
{
public:
	SLATE_BEGIN_ARGS(SGlyphuxText) {}
	SLATE_END_ARGS()

	void Construct(const FArguments& InArgs, UGlyphuxTextBlockBase* InOwner);

	/** The text changed: lay out and paint again. */
	void TextChanged(bool bAnimated);

	virtual int32 OnPaint(const FPaintArgs& Args, const FGeometry& AllottedGeometry, const FSlateRect& MyCullingRect,
		FSlateWindowElementList& OutDrawElements, int32 LayerId, const FWidgetStyle& InWidgetStyle, bool bParentEnabled) const override;
	virtual FReply OnMouseButtonDown(const FGeometry& MyGeometry, const FPointerEvent& MouseEvent) override;
	virtual FReply OnMouseMove(const FGeometry& MyGeometry, const FPointerEvent& MouseEvent) override;
	virtual void OnMouseLeave(const FPointerEvent& MouseEvent) override;
	virtual FCursorReply OnCursorQuery(const FGeometry& MyGeometry, const FPointerEvent& CursorEvent) const override;

protected:
	virtual FVector2D ComputeDesiredSize(float LayoutScaleMultiplier) const override;

private:
	FString LinkUnder(const FGeometry& MyGeometry, const FPointerEvent& Event) const;

	TWeakObjectPtr<UGlyphuxTextBlockBase> Owner;
	bool bOverLink = false;
};
