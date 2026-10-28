#include "GlyphuxTextBlockBase.h"

#include "GlyphuxDrawData.h"
#include "GlyphuxSubsystem.h"
#include "Materials/MaterialInstanceDynamic.h"
#include "SGlyphuxText.h"

#define LOCTEXT_NAMESPACE "Glyphux"

UGlyphuxTextBlockBase::UGlyphuxTextBlockBase(const FObjectInitializer& ObjectInitializer)
	: Super(ObjectInitializer)
{
}

TSharedRef<SWidget> UGlyphuxTextBlockBase::RebuildWidget()
{
	MyText = SNew(SGlyphuxText, this);
	bTextDirty = true;
	return MyText.ToSharedRef();
}

void UGlyphuxTextBlockBase::ReleaseSlateResources(bool bReleaseChildren)
{
	Super::ReleaseSlateResources(bReleaseChildren);
	MyText.Reset();
	ReleaseText();
	bTextDirty = true;
}

#if WITH_EDITOR
const FText UGlyphuxTextBlockBase::GetPaletteCategory()
{
	return LOCTEXT("Palette", "Glyphux");
}
#endif

void UGlyphuxTextBlockBase::PrepareToPaint(float Scale, float Width)
{
	const bool bRescaled = FMath::Abs(Scale - DrawnScale) > 1e-4f * Scale;
	const bool bResized = FMath::Abs(Width - DrawnWidth) > 0.01f;
	if (!bTextDirty && !bRescaled && !bResized)
	{
		return;
	}
	bTextDirty = false;
	DrawnScale = Scale;
	DrawnWidth = Width;
	// Rust decides whether anything needs laying out or drawing again.
	RefreshText(Scale, Width);
}

void UGlyphuxTextBlockBase::SetDrawData(TConstArrayView<float> Quads, TConstArrayView<float> Records, TConstArrayView<float> Tables, float PxRange, bool bAnimated)
{
	if (!Draw)
	{
		Draw = NewObject<UGlyphuxDrawData>(this);
	}
	Draw->Update(Quads, Records, Tables, PxRange, bAnimated);
	UGlyphuxSubsystem* Subsystem = UGlyphuxSubsystem::Get();
	Brush.SetResourceObject(Subsystem ? Draw->GetMaterial(Subsystem->GetUIMaterial()) : nullptr);
	if (MyText.IsValid())
	{
		MyText->TextChanged(bAnimated);
	}
}

void UGlyphuxTextBlockBase::SetContentSize(float Width, float Height)
{
	const FVector2D Size(Width, Height);
	if (!Size.Equals(ContentSize, 0.01))
	{
		ContentSize = Size;
		if (MyText.IsValid())
		{
			MyText->Invalidate(EInvalidateWidgetReason::Layout);
		}
	}
}

void UGlyphuxTextBlockBase::InvalidateText()
{
	bTextDirty = true;
	if (MyText.IsValid())
	{
		MyText->Invalidate(EInvalidateWidgetReason::Paint);
	}
}

#undef LOCTEXT_NAMESPACE
