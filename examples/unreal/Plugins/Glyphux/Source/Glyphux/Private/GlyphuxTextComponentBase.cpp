#include "GlyphuxTextComponentBase.h"

#include "Engine/Engine.h"
#include "Engine/GameViewportClient.h"
#include "Engine/World.h"
#include "Generated/GlyphuxRecords.h"
#include "GlyphuxDrawData.h"
#include "GlyphuxSubsystem.h"
#include "Materials/MaterialInstanceDynamic.h"
#include "UnrealClient.h"

UGlyphuxTextComponentBase::UGlyphuxTextComponentBase(const FObjectInitializer& ObjectInitializer)
	: Super(ObjectInitializer)
{
	PrimaryComponentTick.bCanEverTick = true;
	PrimaryComponentTick.bStartWithTickEnabled = true;
	bTickInEditor = true;
	CastShadow = false;
	SetCollisionEnabled(ECollisionEnabled::NoCollision);
	SetGenerateOverlapEvents(false);
}

void UGlyphuxTextComponentBase::OnRegister()
{
	Super::OnRegister();
	InvalidateText();
}

void UGlyphuxTextComponentBase::OnUnregister()
{
	ReleaseText();
	Super::OnUnregister();
}

#if WITH_EDITOR
void UGlyphuxTextComponentBase::PostEditChangeProperty(FPropertyChangedEvent& PropertyChangedEvent)
{
	Super::PostEditChangeProperty(PropertyChangedEvent);
	const FName Name = PropertyChangedEvent.GetMemberPropertyName();
	if (Name == GET_MEMBER_NAME_CHECKED(UGlyphuxTextComponentBase, PixelSize)
		|| Name == GET_MEMBER_NAME_CHECKED(UGlyphuxTextComponentBase, bNoDepthTest))
	{
		BuildMesh(0.0f, false);
		InvalidateText();
	}
}
#endif

void UGlyphuxTextComponentBase::SetPixelSize(float InPixelSize)
{
	PixelSize = FMath::Max(InPixelSize, 0.0001f);
	BuildMesh(0.0f, false);
	InvalidateText();
}

void UGlyphuxTextComponentBase::SetNoDepthTest(bool bInNoDepthTest)
{
	bNoDepthTest = bInNoDepthTest;
	BuildMesh(0.0f, false);
}

void UGlyphuxTextComponentBase::SetDrawText(bool bInDrawText)
{
	bDrawText = bInDrawText;
	BuildMesh(0.0f, false);
}

UGlyphuxDrawData* UGlyphuxTextComponentBase::EnsureDraw()
{
	if (!Draw)
	{
		Draw = NewObject<UGlyphuxDrawData>(this);
	}
	return Draw;
}

TArray<FGlyphuxGlyph> UGlyphuxTextComponentBase::GetGlyphs()
{
	TArray<FGlyphuxGlyph> Glyphs;
	if (!Draw)
	{
		return Glyphs;
	}
	const FTransform& Transform = GetComponentTransform();
	const FVector Scale = Transform.GetScale3D();
	// Layout x along the component's -Y, layout y along its -Z.
	auto Local = [this](const FVector2f& P) { return FVector(0.0, -P.X * PixelSize, -P.Y * PixelSize); };
	const TArray<FGlyphuxSurfaceQuad>& Quads = Draw->GetSurfaceQuadList();
	Glyphs.Reserve(Quads.Num());
	for (int32 I = 0; I < Quads.Num(); ++I)
	{
		const FGlyphuxSurfaceQuad& Q = Quads[I];
		const FVector2f Center = Q.TopLeft + 0.5f * ((Q.TopRight - Q.TopLeft) + (Q.BottomLeft - Q.TopLeft));
		FGlyphuxGlyph& G = Glyphs.AddDefaulted_GetRef();
		G.Position = Transform.TransformPosition(Local(Center));
		G.Size = FVector2D((Q.TopRight - Q.TopLeft).Size() * PixelSize * FMath::Abs(Scale.Y),
			(Q.BottomLeft - Q.TopLeft).Size() * PixelSize * FMath::Abs(Scale.Z));
		G.Rotation = Transform.GetRotation();
		G.Quad = I;
	}
	return Glyphs;
}

UMaterialInstanceDynamic* UGlyphuxTextComponentBase::GetGlyphMaterial(UMaterialInterface* Base)
{
	UGlyphuxSubsystem* Subsystem = UGlyphuxSubsystem::Get();
	if (!Base && Subsystem)
	{
		Base = Subsystem->GetGlyphMaterial();
	}
	return EnsureDraw()->GetMaterial(Base, true);
}

UMaterialInstanceDynamic* UGlyphuxTextComponentBase::GetSurfaceMaterial(UMaterialInterface* Base)
{
	UGlyphuxSubsystem* Subsystem = UGlyphuxSubsystem::Get();
	if (!Base && Subsystem)
	{
		Base = Subsystem->GetSurfaceMaterial();
	}
	return EnsureDraw()->GetMaterial(Base, true);
}

void UGlyphuxTextComponentBase::BindToMaterial(UMaterialInstanceDynamic* Target)
{
	EnsureDraw()->Bind(Target);
}

void UGlyphuxTextComponentBase::InvalidateText()
{
	// Rust decides in RefreshText, every frame: nothing more to do here than to make sure the
	// component ticks.
	SetComponentTickEnabled(true);
}

float UGlyphuxTextComponentBase::MeasureScale() const
{
	const UWorld* World = GetWorld();
	const FVector Origin = GetComponentLocation();
	float Distance = 1000.0f;
	if (World && World->ViewLocationsRenderedLastFrame.Num() > 0)
	{
		Distance = TNumericLimits<float>::Max();
		for (const FVector& View : World->ViewLocationsRenderedLastFrame)
		{
			Distance = FMath::Min(Distance, static_cast<float>(FVector::Dist(View, Origin)));
		}
	}
	// A 90 degree field of view: half the viewport's height over the distance.
	float Height = 1080.0f;
	if (GEngine && GEngine->GameViewport && GEngine->GameViewport->Viewport)
	{
		Height = FMath::Max(1.0f, static_cast<float>(GEngine->GameViewport->Viewport->GetSizeXY().Y));
	}
	const float PixelsPerWorldUnit = 0.5f * Height / FMath::Max(Distance, 1.0f);
	return PixelsPerWorldUnit * PixelSize * static_cast<float>(GetComponentTransform().GetMaximumAxisScale());
}

void UGlyphuxTextComponentBase::TickComponent(float DeltaTime, ELevelTick TickType, FActorComponentTickFunction* ThisTickFunction)
{
	Super::TickComponent(DeltaTime, TickType, ThisTickFunction);
	if (!IsVisible())
	{
		return;
	}
	if (bBillboard)
	{
		FaceCamera();
	}
	RefreshText(FixedScale > 0.0f ? FixedScale : MeasureScale());
	if (Draw && Draw->IsAnimated())
	{
		const UWorld* World = GetWorld();
		BuildMesh(World ? World->GetTimeSeconds() : 0.0f, true);
	}
}

void UGlyphuxTextComponentBase::FaceCamera()
{
	const UWorld* World = GetWorld();
	if (!World || !World->IsGameWorld() || World->ViewLocationsRenderedLastFrame.Num() == 0)
	{
		return;
	}
	const FVector ToView = World->ViewLocationsRenderedLastFrame[0] - GetComponentLocation();
	if (!ToView.IsNearlyZero())
	{
		SetWorldRotation(ToView.Rotation());
	}
}

void UGlyphuxTextComponentBase::SetDrawData(TConstArrayView<float> Quads, TConstArrayView<float> Records, TConstArrayView<float> Tables, float PxRange, bool bAnimated)
{
	EnsureDraw()->Update(Quads, Records, Tables, PxRange, bAnimated);
	const UWorld* World = GetWorld();
	BuildMesh(World ? World->GetTimeSeconds() : 0.0f, false);
	OnTextUpdated.Broadcast();
}

void UGlyphuxTextComponentBase::SetContentBox(float X, float Y, float Width, float Height)
{
	Box = FVector4f(X, Y, Width, Height);
	EnsureDraw()->SetBox(Box);
}

void UGlyphuxTextComponentBase::BuildMesh(float Time, bool bPositionsOnly)
{
	const int32 Count = Draw && bDrawText ? Draw->NumQuads() : 0;
	if (Count == 0)
	{
		ClearAllMeshSections();
		return;
	}
	const TArray<float>& Quads = Draw->GetQuads();
	TArray<FVector> Positions;
	TArray<FVector2D> UV0, UV1;
	TArray<int32> Triangles;
	Positions.Reserve(Count * 4);
	for (int32 I = 0; I < Count; ++I)
	{
		const float* Q = Quads.GetData() + I * GlyphuxRecords::QuadFloats;
		const FVector2f Move = Draw->IsAnimated() ? Draw->Animation(I, Time) : FVector2f(0.0f, 0.0f);
		// The corners' place in the quad, and the quad's index split so that both halves stay
		// exact at half precision (a procedural mesh's): the world material reads the quad's
		// texture coordinates, record and pass from the draw quads texture.
		const float U[4] = {0.0f, 1.0f, 0.0f, 1.0f};
		const float V[4] = {0.0f, 0.0f, 1.0f, 1.0f};
		const FVector2D QuadIndex(static_cast<double>(I % 2048), static_cast<double>(I / 2048));
		const int32 Base = Positions.Num();
		for (int32 C = 0; C < 4; ++C)
		{
			const float X = Q[2 * C] + Move.X;
			const float Y = Q[2 * C + 1] + Move.Y;
			// Layout x to the left of +X (reading from in front, the +X side), y down.
			Positions.Add(FVector(0.0, -X * PixelSize, -Y * PixelSize));
			if (!bPositionsOnly)
			{
				UV0.Add(FVector2D(U[C], V[C]));
				UV1.Add(QuadIndex);
			}
		}
		if (!bPositionsOnly)
		{
			Triangles.Append({Base, Base + 2, Base + 1, Base + 2, Base + 3, Base + 1});
		}
	}
	if (bPositionsOnly && GetNumSections() > 0)
	{
		UpdateMeshSection(0, Positions, TArray<FVector>(), TArray<FVector2D>(), TArray<FColor>(), TArray<FProcMeshTangent>());
		return;
	}
	TArray<FVector> Normals;
	Normals.Init(FVector(1.0, 0.0, 0.0), Positions.Num());
	TArray<FLinearColor> Colors;
	Colors.Init(FLinearColor::White, Positions.Num());
	CreateMeshSection_LinearColor(0, Positions, Triangles, Normals, UV0, UV1, TArray<FVector2D>(), TArray<FVector2D>(), Colors, TArray<FProcMeshTangent>(), false);
	if (UGlyphuxSubsystem* Subsystem = UGlyphuxSubsystem::Get())
	{
		SetMaterial(0, Draw->GetMaterial(Subsystem->GetWorldMaterial(!bNoDepthTest)));
	}
}
