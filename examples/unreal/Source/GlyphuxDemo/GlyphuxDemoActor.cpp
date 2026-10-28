#include "GlyphuxDemoActor.h"

#include "Components/StaticMeshComponent.h"
#include "Engine/StaticMesh.h"
#include "GlyphuxNiagaraLibrary.h"
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
		GetWorldTimerManager().SetTimer(Timer, this, &AGlyphuxDemoActor::BreakTitle, ShatterEvery, true, ShatterEvery * 0.5f);
	}
}

void AGlyphuxDemoActor::Tick(float DeltaSeconds)
{
	Super::Tick(DeltaSeconds);
	Cylinder->AddLocalRotation(FRotator(0.0, CylinderSpeed * DeltaSeconds, 0.0));
}

void AGlyphuxDemoActor::BreakTitle()
{
	if (!ShatterSystem || UGlyphuxNiagaraLibrary::SetNiagaraGlyphs(Shatter, Title, Shards) == 0)
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
