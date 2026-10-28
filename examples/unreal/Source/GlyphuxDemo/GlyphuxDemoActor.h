// The demo: drop one in a level (Place Actors, search "Glyphux Demo") and press Play.
//
// * A title, a Glyphux Text component with BBCode, glow and a shadow.
// * A cylinder turning, with a text wrapped around it: the surface material, drawn from a
//   second text component that only lays the text out.
// * With Shatter System set (a Niagara system built as NIAGARA.md says) and, for fragments,
//   Shards: every Shatter Every seconds the title breaks apart, then comes back.

#pragma once

#include "CoreMinimal.h"
#include "GameFramework/Actor.h"
#include "GlyphuxDemoActor.generated.h"

class UGlyphuxShardSet;
class UGlyphuxTextComponent;
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

	/** The fragments (DA_GlyphuxShards); none: a particle per glyph. */
	UPROPERTY(EditAnywhere, BlueprintReadOnly, Category = "Demo")
	TObjectPtr<UGlyphuxShardSet> Shards;

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

	FTimerHandle Timer;
};
