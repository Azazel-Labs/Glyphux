#include "GlyphuxDemoActor.h"

#include "Components/InstancedStaticMeshComponent.h"
#include "Components/StaticMeshComponent.h"
#include "Engine/StaticMesh.h"
#include "GlyphuxNiagaraLibrary.h"
#include "GlyphuxShardSet.h"
#include "Materials/MaterialInstanceDynamic.h"
#include "NiagaraComponent.h"
#include "NiagaraSystem.h"
#include "Ferreal/GlyphuxTextComponent.h"
#include "TimerManager.h"
#include "UObject/ConstructorHelpers.h"

AGlyphuxDemoActor::AGlyphuxDemoActor()
{
	PrimaryActorTick.bCanEverTick = true;
	RootComponent = CreateDefaultSubobject<USceneComponent>(TEXT("Root"));

	Title = CreateDefaultSubobject<UGlyphuxTextComponent>(TEXT("Title"));
	Title->SetupAttachment(RootComponent);
	Title->SetRelativeLocation(FVector(0.0, 0.0, 260.0));
	// The properties directly: the setters also tell the Rust, which a constructor must not.
	Title->bRich = true;
	Title->Text = TEXT("[b]Glyphux[/b] in [color=#7fd4ff]Unreal[/color]\n[wave amp=40 freq=6]sharp at any size[/wave]");
	Title->FontSize = 56.0f;
	Title->GlowColor = FLinearColor(1.0f, 0.45f, 0.1f, 0.8f);
	Title->GlowRadius = 6.0f;
	Title->ShadowColor = FLinearColor(0.0f, 0.0f, 0.0f, 0.6f);

	Cylinder = CreateDefaultSubobject<UStaticMeshComponent>(TEXT("Cylinder"));
	Cylinder->SetupAttachment(RootComponent);
	Cylinder->SetRelativeScale3D(FVector(1.5, 1.5, 1.2));
	static ConstructorHelpers::FObjectFinder<UStaticMesh> CylinderMesh(TEXT("/Engine/BasicShapes/Cylinder.Cylinder"));
	if (CylinderMesh.Succeeded())
	{
		Cylinder->SetStaticMesh(CylinderMesh.Object);
	}

	CylinderText = CreateDefaultSubobject<UGlyphuxTextComponent>(TEXT("CylinderText"));
	CylinderText->SetupAttachment(Cylinder);
	CylinderText->bDrawText = false;
	// Fields for twice the font size: the text spans the cylinder's texture coordinates, a
	// few hundred screen pixels across at a normal distance.
	CylinderText->FixedScale = 2.0f;
	CylinderText->bRich = true;
	CylinderText->Text = TEXT("[color=#ffd166]around[/color] and [i]around[/i], text on any mesh,");
	CylinderText->FontSize = 40.0f;
	CylinderText->OutlineWidth = 2.0f;

	Shatter = CreateDefaultSubobject<UNiagaraComponent>(TEXT("Shatter"));
	Shatter->SetupAttachment(RootComponent);
	Shatter->SetAutoActivate(false);

	// Where Content/Python/glyphux_materials.py creates it.
	FragmentMaterial = TSoftObjectPtr<UMaterialInterface>(FSoftObjectPath(TEXT("/Glyphux/Materials/M_GlyphuxGlyphInstanced.M_GlyphuxGlyphInstanced")));
}

void AGlyphuxDemoActor::OnConstruction(const FTransform& Transform)
{
	Super::OnConstruction(Transform);
	CylinderText->OnTextUpdated.AddUniqueDynamic(this, &AGlyphuxDemoActor::BindCylinder);
	BindCylinder();
}

void AGlyphuxDemoActor::BindCylinder()
{
	// The surface material maps texture coordinates 0 to 1 onto the text's box.
	if (UMaterialInstanceDynamic* Material = CylinderText->GetSurfaceMaterial())
	{
		Cylinder->SetMaterial(0, Material);
	}
}

void AGlyphuxDemoActor::BeginPlay()
{
	Super::BeginPlay();
	if (ShatterSystem)
	{
		Shatter->SetAsset(ShatterSystem);
	}
	if (ShatterSystem || Shards)
	{
		GetWorldTimerManager().SetTimer(Timer, this, &AGlyphuxDemoActor::BreakTitle, ShatterEvery, true, ShatterEvery * 0.5f);
	}
}

void AGlyphuxDemoActor::Tick(float DeltaSeconds)
{
	Super::Tick(DeltaSeconds);
	Cylinder->AddLocalRotation(FRotator(0.0, CylinderSpeed * DeltaSeconds, 0.0));
	if (FragmentAge >= 0.0f)
	{
		MoveFragments(DeltaSeconds);
	}
}

void AGlyphuxDemoActor::BreakTitle()
{
	if (!ShatterSystem)
	{
		if (FragmentAge < 0.0f && BreakIntoFragments())
		{
			Title->SetDrawText(false);
		}
		return;
	}
	if (UGlyphuxNiagaraLibrary::SetNiagaraGlyphs(Shatter, Title, Shards) == 0)
	{
		return;
	}
	// The particles take the glyphs' place: the title hides while they fly, still laid out.
	Title->SetDrawText(false);
	Shatter->ResetSystem();
	Shatter->Activate(true);
	FTimerHandle Back;
	GetWorldTimerManager().SetTimer(Back, this, &AGlyphuxDemoActor::RestoreTitle, ShatterEvery * 0.6f, false);
}

void AGlyphuxDemoActor::RestoreTitle()
{
	Title->SetDrawText(true);
}

bool AGlyphuxDemoActor::BreakIntoFragments()
{
	if (!Shards || Shards->Shards.Num() == 0)
	{
		return false;
	}
	const TArray<FGlyphuxGlyph> Glyphs = Title->GetGlyphs();
	UMaterialInterface* Base = FragmentMaterial.LoadSynchronous();
	if (Glyphs.Num() == 0 || !Base)
	{
		return false;
	}
	UMaterialInstanceDynamic* Material = Title->GetGlyphMaterial(Base);

	// An instanced mesh per fragment shape, placed in world space: the instances are the
	// glyphs' fragments, their quad index in the instance's custom data.
	if (FragmentMeshes.Num() != Shards->Shards.Num())
	{
		for (UInstancedStaticMeshComponent* Old : FragmentMeshes)
		{
			Old->DestroyComponent();
		}
		FragmentMeshes.Reset();
		for (const FGlyphuxShard& Shard : Shards->Shards)
		{
			UInstancedStaticMeshComponent* Mesh = NewObject<UInstancedStaticMeshComponent>(this);
			Mesh->SetStaticMesh(Shard.Mesh);
			Mesh->SetUsingAbsoluteLocation(true);
			Mesh->SetUsingAbsoluteRotation(true);
			Mesh->SetUsingAbsoluteScale(true);
			Mesh->SetCollisionEnabled(ECollisionEnabled::NoCollision);
			Mesh->SetCastShadow(false);
			Mesh->NumCustomDataFloats = 1;
			Mesh->SetupAttachment(RootComponent);
			Mesh->RegisterComponent();
			FragmentMeshes.Add(Mesh);
		}
	}
	for (UInstancedStaticMeshComponent* Mesh : FragmentMeshes)
	{
		Mesh->ClearInstances();
		Mesh->SetMaterial(0, Material);
	}

	Fragments.Reset();
	for (const FGlyphuxGlyph& G : Glyphs)
	{
		const FVector Toward = G.Rotation.GetForwardVector();
		const FVector Up = G.Rotation.GetUpVector();
		for (int32 S = 0; S < Shards->Shards.Num(); ++S)
		{
			// The fragment's center in the glyph's square, u along -Y and v along -Z, as
			// SetNiagaraGlyphs places it.
			const FVector2D C = Shards->Shards[S].Center;
			FFragment F;
			F.Mesh = S;
			F.Quad = static_cast<float>(G.Quad);
			F.Rest = G.Position + G.Rotation.RotateVector(FVector(0.0, -(C.X - 0.5) * G.Size.X, -(C.Y - 0.5) * G.Size.Y));
			F.Rotation = G.Rotation;
			F.Scale = FVector(1.0, G.Size.X, G.Size.Y);
			// Away from the glyph's center, toward the viewer and up a little.
			F.Velocity = (F.Rest - G.Position).GetSafeNormal() * FMath::FRandRange(120.0, 320.0)
				+ Toward * FMath::FRandRange(80.0, 260.0) + Up * FMath::FRandRange(60.0, 220.0);
			F.SpinAxis = FMath::VRand();
			F.SpinRate = FMath::FRandRange(2.0f, 7.0f);
			UInstancedStaticMeshComponent* Mesh = FragmentMeshes[S];
			F.Instance = Mesh->AddInstance(FTransform(F.Rotation, F.Rest, F.Scale), true);
			Mesh->SetCustomDataValue(F.Instance, 0, F.Quad);
			Fragments.Add(F);
		}
	}
	FragmentAge = 0.0f;
	return true;
}

void AGlyphuxDemoActor::MoveFragments(float DeltaSeconds)
{
	// They fly for the first part, then ease back to where they broke off, and the title
	// takes their place.
	FragmentAge += DeltaSeconds;
	const float Fly = ShatterEvery * 0.35f;
	const float Back = ShatterEvery * 0.25f;
	const FVector Gravity(0.0, 0.0, -300.0);
	if (FragmentAge >= Fly + Back)
	{
		for (UInstancedStaticMeshComponent* Mesh : FragmentMeshes)
		{
			Mesh->ClearInstances();
		}
		Fragments.Reset();
		FragmentAge = -1.0f;
		RestoreTitle();
		return;
	}

	const float T = FMath::Min(FragmentAge, Fly);
	const float Home = FragmentAge > Fly ? FMath::SmoothStep(0.0f, 1.0f, (FragmentAge - Fly) / Back) : 0.0f;
	TArray<TArray<FTransform>> Transforms;
	Transforms.SetNum(FragmentMeshes.Num());
	for (int32 M = 0; M < FragmentMeshes.Num(); ++M)
	{
		Transforms[M].SetNum(FragmentMeshes[M]->GetInstanceCount());
	}
	for (const FFragment& F : Fragments)
	{
		const FVector Flown = F.Rest + F.Velocity * T + 0.5 * Gravity * T * T;
		const FQuat Spin(F.SpinAxis, F.SpinRate * T * (1.0f - Home));
		Transforms[F.Mesh][F.Instance] = FTransform(Spin * F.Rotation, FMath::Lerp(Flown, F.Rest, static_cast<double>(Home)), F.Scale);
	}
	for (int32 M = 0; M < FragmentMeshes.Num(); ++M)
	{
		FragmentMeshes[M]->BatchUpdateInstancesTransforms(0, Transforms[M], true, true, true);
	}
}
