// The demo: drop one in a level (Place Actors, search "Glyphux Demo") and press Play.
//
// * A title, a Glyphux Text component with BBCode, glow and a shadow.
// * A cylinder turning, with a text wrapped around it: the surface material, drawn from a
//   second text component that only lays the text out.
// * Every Shatter Every seconds the title breaks apart, then comes back: through Shatter
//   System when set (a Niagara system built as NIAGARA.md says), else its Shards as
//   instanced static meshes that fly apart and reassemble, with no Niagara system to build.

#pragma once

#include "CoreMinimal.h"
#include "GameFramework/Actor.h"
#include "GlyphuxDemoActor.generated.h"

class UGlyphuxShardSet;
class UGlyphuxTextComponent;
class UInstancedStaticMeshComponent;
class UMaterialInterface;
class UNiagaraComponent;
class UNiagaraSystem;
class UStaticMeshComponent;

UCLASS(meta = (DisplayName = "Glyphux Demo"))
class GLYPHUXDEMO_API AGlyphuxDemoActor : public AActor
{
	GENERATED_BODY()

public:
	AGlyphuxDemoActor();

	UPROPERTY(VisibleAnywhere, BlueprintReadOnly, Category = "Demo")
	TObjectPtr<UGlyphuxTextComponent> Title;

	UPROPERTY(VisibleAnywhere, BlueprintReadOnly, Category = "Demo")
	TObjectPtr<UStaticMeshComponent> Cylinder;

	/** Lays out the cylinder's text (not drawn itself). */
	UPROPERTY(VisibleAnywhere, BlueprintReadOnly, Category = "Demo")
	TObjectPtr<UGlyphuxTextComponent> CylinderText;

	UPROPERTY(VisibleAnywhere, BlueprintReadOnly, Category = "Demo")
	TObjectPtr<UNiagaraComponent> Shatter;

	/** The system breaking the title apart (NIAGARA.md); none: the title stays. */
	UPROPERTY(EditAnywhere, BlueprintReadOnly, Category = "Demo")
	TObjectPtr<UNiagaraSystem> ShatterSystem;

	/**
	 * The fragments (DA_GlyphuxShards). With Shatter System, none is a particle per glyph;
	 * without it, none keeps the title whole.
	 */
	UPROPERTY(EditAnywhere, BlueprintReadOnly, Category = "Demo")
	TObjectPtr<UGlyphuxShardSet> Shards;

	/** Without Shatter System: the fragments' material, the glyph material for instanced static meshes. */
	UPROPERTY(EditAnywhere, BlueprintReadOnly, Category = "Demo")
	TSoftObjectPtr<UMaterialInterface> FragmentMaterial;

	UPROPERTY(EditAnywhere, BlueprintReadOnly, Category = "Demo", meta = (ClampMin = "1"))
	float ShatterEvery = 6.0f;

	/** Degrees a second. */
	UPROPERTY(EditAnywhere, BlueprintReadOnly, Category = "Demo")
	float CylinderSpeed = 20.0f;

	/** Breaks the title apart now. */
	UFUNCTION(BlueprintCallable, Category = "Demo")
	void BreakTitle();

	virtual void OnConstruction(const FTransform& Transform) override;
	virtual void BeginPlay() override;
	virtual void Tick(float DeltaSeconds) override;

private:
	UFUNCTION()
	void BindCylinder();

	void RestoreTitle();

	/** Without Shatter System: the title's glyphs, cut into Shards, start flying. */
	bool BreakIntoFragments();
	void MoveFragments(float DeltaSeconds);

	FTimerHandle Timer;

	/** One fragment of a glyph: where it rests (as the title), and how it flies. */
	struct FFragment
	{
		int32 Mesh = 0;
		int32 Instance = 0;
		float Quad = 0.0f;
		FVector Rest = FVector::ZeroVector;
		FQuat Rotation = FQuat::Identity;
		FVector Scale = FVector::OneVector;
		FVector Velocity = FVector::ZeroVector;
		FVector SpinAxis = FVector::UpVector;
		float SpinRate = 0.0f;
	};

	TArray<FFragment> Fragments;

	/** One per shard mesh, in Shards' order. */
	UPROPERTY(Transient)
	TArray<TObjectPtr<UInstancedStaticMeshComponent>> FragmentMeshes;

	/** Seconds since the fragments broke off; negative while the title is whole. */
	float FragmentAge = -1.0f;
};
