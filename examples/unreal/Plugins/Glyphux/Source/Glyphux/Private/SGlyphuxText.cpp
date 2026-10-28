#include "SGlyphuxText.h"

#include "Framework/Application/SlateApplication.h"
#include "Generated/GlyphuxRecords.h"
#include "GlyphuxDrawData.h"
#include "GlyphuxTextBlockBase.h"
#include "Rendering/DrawElements.h"
#include "Rendering/SlateRenderer.h"

void SGlyphuxText::Construct(const FArguments& InArgs, UGlyphuxTextBlockBase* InOwner)
{
	Owner = InOwner;
}

void SGlyphuxText::TextChanged(bool bAnimated)
{
	// Animated text (wave, shake) moves every frame; rainbow text changes color in the material.
	ForceVolatile(bAnimated);
	Invalidate(EInvalidateWidgetReason::Layout);
}

FVector2D SGlyphuxText::ComputeDesiredSize(float) const
{
	const UGlyphuxTextBlockBase* O = Owner.Get();
	return O ? O->GetContentSize() : FVector2D::ZeroVector;
}

int32 SGlyphuxText::OnPaint(const FPaintArgs& Args, const FGeometry& AllottedGeometry, const FSlateRect& MyCullingRect,
	FSlateWindowElementList& OutDrawElements, int32 LayerId, const FWidgetStyle& InWidgetStyle, bool bParentEnabled) const
{
	UGlyphuxTextBlockBase* O = Owner.Get();
	if (!O)
	{
		return LayerId;
	}
	// Screen pixels per Slate unit: the DPI scale and every render transform above.
	const FSlateRenderTransform& Transform = AllottedGeometry.GetAccumulatedRenderTransform();
	const float Scale = FMath::Max(Transform.TransformVector(FVector2f(1.0f, 0.0f)).Size(), Transform.TransformVector(FVector2f(0.0f, 1.0f)).Size());
	O->PrepareToPaint(Scale, AllottedGeometry.GetLocalSize().X);

	const UGlyphuxDrawData* Draw = O->GetDrawData();
	const FSlateBrush& Brush = O->GetBrush();
	if (!Draw || Draw->NumQuads() == 0 || !Brush.GetResourceObject())
	{
		return LayerId;
	}
	const FSlateResourceHandle Handle = FSlateApplication::Get().GetRenderer()->GetResourceHandle(Brush);
	const FColor Tint = InWidgetStyle.GetColorAndOpacityTint().ToFColor(true);
	const float Time = static_cast<float>(FSlateApplication::Get().GetCurrentTime());
	const bool bAnimated = Draw->IsAnimated();
	const TArray<float>& Quads = Draw->GetQuads();
	const int32 Count = Draw->NumQuads();

	TArray<FSlateVertex> Vertices;
	TArray<SlateIndex> Indices;
	Vertices.Reserve(Count * 4);
	Indices.Reserve(Count * 6);
	for (int32 I = 0; I < Count; ++I)
	{
		const float* Q = Quads.GetData() + I * GlyphuxRecords::QuadFloats;
		const FVector2f Move = bAnimated ? Draw->Animation(I, Time) : FVector2f(0.0f, 0.0f);
		// Corners: top-left, top-right, bottom-left, bottom-right; texture coordinates from
		// the top-left and bottom-right ones.
		const float U[4] = {Q[8], Q[10], Q[8], Q[10]};
		const float V[4] = {Q[9], Q[9], Q[11], Q[11]};
		const SlateIndex Base = static_cast<SlateIndex>(Vertices.Num());
		for (int32 C = 0; C < 4; ++C)
		{
			const FVector2f Local(Q[2 * C] + Move.X, Q[2 * C + 1] + Move.Y);
			FSlateVertex Vertex;
			Vertex.Position = Transform.TransformPoint(Local);
			// TexCoord 0: the glyph's texture coordinates; TexCoord 1: its record and pass.
			Vertex.TexCoords[0] = U[C];
			Vertex.TexCoords[1] = V[C];
			Vertex.TexCoords[2] = Q[12];
			Vertex.TexCoords[3] = Q[13];
			Vertex.MaterialTexCoords = Local;
			Vertex.Color = Tint;
			Vertex.SecondaryColor = FColor::White;
			Vertex.PixelSize[0] = 0;
			Vertex.PixelSize[1] = 0;
			Vertices.Add(Vertex);
		}
		Indices.Append({Base, static_cast<SlateIndex>(Base + 1), static_cast<SlateIndex>(Base + 2),
			static_cast<SlateIndex>(Base + 2), static_cast<SlateIndex>(Base + 1), static_cast<SlateIndex>(Base + 3)});
	}
	FSlateDrawElement::MakeCustomVerts(OutDrawElements, LayerId, Handle, Vertices, Indices, nullptr, 0, 0);
	return LayerId;
}

FString SGlyphuxText::LinkUnder(const FGeometry& MyGeometry, const FPointerEvent& Event) const
{
	const UGlyphuxTextBlockBase* O = Owner.Get();
	return O ? O->LinkAt(MyGeometry.AbsoluteToLocal(Event.GetScreenSpacePosition())) : FString();
}

FReply SGlyphuxText::OnMouseButtonDown(const FGeometry& MyGeometry, const FPointerEvent& MouseEvent)
{
	if (MouseEvent.GetEffectingButton() == EKeys::LeftMouseButton)
	{
		const FString Link = LinkUnder(MyGeometry, MouseEvent);
		if (!Link.IsEmpty())
		{
			if (UGlyphuxTextBlockBase* O = Owner.Get())
			{
				O->OnLinkClicked.Broadcast(Link);
			}
			return FReply::Handled();
		}
	}
	return FReply::Unhandled();
}

FReply SGlyphuxText::OnMouseMove(const FGeometry& MyGeometry, const FPointerEvent& MouseEvent)
{
	bOverLink = !LinkUnder(MyGeometry, MouseEvent).IsEmpty();
	return FReply::Unhandled();
}

void SGlyphuxText::OnMouseLeave(const FPointerEvent& MouseEvent)
{
	bOverLink = false;
	SLeafWidget::OnMouseLeave(MouseEvent);
}

FCursorReply SGlyphuxText::OnCursorQuery(const FGeometry& MyGeometry, const FPointerEvent& CursorEvent) const
{
	return bOverLink ? FCursorReply::Cursor(EMouseCursor::Hand) : FCursorReply::Unhandled();
}
