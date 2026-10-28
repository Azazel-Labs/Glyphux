#include "GlyphuxNiagaraLibrary.h"

#include "GlyphuxShardSet.h"
#include "GlyphuxTextComponentBase.h"
#include "Materials/MaterialInstanceDynamic.h"
#include "NiagaraComponent.h"
#include "NiagaraDataInterfaceArrayFunctionLibrary.h"

int32 UGlyphuxNiagaraLibrary::SetNiagaraGlyphs(UNiagaraComponent* System, UGlyphuxTextComponentBase* Text, UGlyphuxShardSet* Shards,
	UMaterialInterface* Material, bool bLocalSpace)
{
	if (!System || !Text)
	{
		return 0;
	}
	const TArray<FGlyphuxGlyph> Glyphs = Text->GetGlyphs();
	const FTransform ToSystem = bLocalSpace ? System->GetComponentTransform().Inverse() : FTransform::Identity;
	const FQuat Rotation = ToSystem.GetRotation() * Text->GetComponentQuat();
	const bool bShards = Shards && Shards->Shards.Num() > 0;
	const int32 PerGlyph = bShards ? Shards->Shards.Num() : 1;

	TArray<FVector> Positions, Centers;
	TArray<FVector2D> Sizes;
	TArray<float> Quads;
	TArray<int32> Meshes;
	const int32 Count = Glyphs.Num() * PerGlyph;
	Positions.Reserve(Count);
	Centers.Reserve(Count);
	Sizes.Reserve(Count);
	Quads.Reserve(Count);
	Meshes.Reserve(Count);
	for (const FGlyphuxGlyph& G : Glyphs)
	{
		const FVector Center = ToSystem.TransformPosition(G.Position);
		for (int32 S = 0; S < PerGlyph; ++S)
		{
			FVector At = Center;
			if (bShards)
			{
				// The fragment's center in the glyph's square, the square's u along -Y and v
				// along -Z (scaled by the glyph's size).
				const FVector2D C = Shards->Shards[S].Center;
				At += Rotation.RotateVector(FVector(0.0, -(C.X - 0.5) * G.Size.X, -(C.Y - 0.5) * G.Size.Y));
			}
			Positions.Add(At);
			Centers.Add(Center);
			Sizes.Add(G.Size);
			Quads.Add(static_cast<float>(G.Quad));
			Meshes.Add(S);
		}
	}

	UNiagaraDataInterfaceArrayFunctionLibrary::SetNiagaraArrayVector(System, TEXT("GlyphuxPositions"), Positions);
	UNiagaraDataInterfaceArrayFunctionLibrary::SetNiagaraArrayVector(System, TEXT("GlyphuxCenters"), Centers);
	UNiagaraDataInterfaceArrayFunctionLibrary::SetNiagaraArrayVector2D(System, TEXT("GlyphuxSizes"), Sizes);
	UNiagaraDataInterfaceArrayFunctionLibrary::SetNiagaraArrayFloat(System, TEXT("GlyphuxQuads"), Quads);
	UNiagaraDataInterfaceArrayFunctionLibrary::SetNiagaraArrayInt32(System, TEXT("GlyphuxMeshIndices"), Meshes);
	System->SetVariableInt(TEXT("GlyphuxCount"), Count);
	System->SetVariableQuat(TEXT("GlyphuxRotation"), Rotation);
	if (UMaterialInstanceDynamic* Instance = Text->GetGlyphMaterial(Material))
	{
		System->SetVariableMaterial(TEXT("GlyphuxMaterial"), Instance);
	}
	return Count;
}
