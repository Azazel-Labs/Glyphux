#include "GlyphuxDrawData.h"

#include "Engine/Texture2D.h"
#include "Generated/GlyphuxRecords.h"
#include "GlyphuxSubsystem.h"
#include "Materials/MaterialInstanceDynamic.h"
#include "TextureResource.h"
#include "UObject/Package.h"

namespace
{
	/**
	 * Writes Texels (four floats each) into Texture, Width texels per row, creating it again
	 * when it has too few rows. A float texture sampled with Load: no filtering, no sRGB.
	 */
	void WriteTexels(UObject* Outer, TObjectPtr<UTexture2D>& Texture, TConstArrayView<float> Values, int32 Width, const TCHAR* Name)
	{
		const int32 Texels = FMath::Max(1, FMath::DivideAndRoundUp(Values.Num(), 4));
		const int32 Rows = FMath::RoundUpToPowerOfTwo(FMath::DivideAndRoundUp(Texels, Width));
		if (!Texture || Texture->GetSizeY() < Rows)
		{
			Texture = UTexture2D::CreateTransient(Width, Rows, PF_A32B32G32R32F, MakeUniqueObjectName(GetTransientPackage(), UTexture2D::StaticClass(), FName(Name)));
			if (!Texture)
			{
				return;
			}
			Texture->SRGB = false;
			Texture->Filter = TF_Nearest;
			Texture->NeverStream = true;
			Texture->UpdateResource();
		}
		// Whole rows, zero padded; the render thread copies them, then frees them.
		const int32 UsedRows = FMath::DivideAndRoundUp(Texels, Width);
		const int32 Floats = UsedRows * Width * 4;
		float* Data = static_cast<float*>(FMemory::MallocZeroed(Floats * sizeof(float)));
		FMemory::Memcpy(Data, Values.GetData(), Values.Num() * sizeof(float));
		FUpdateTextureRegion2D* Region = new FUpdateTextureRegion2D(0, 0, 0, 0, Width, UsedRows);
		Texture->UpdateTextureRegions(0, 1, Region, Width * 16, 16, reinterpret_cast<uint8*>(Data),
			[](uint8* SrcData, const FUpdateTextureRegion2D* Regions)
			{
				FMemory::Free(SrcData);
				delete Regions;
			});
	}

	float Hash(float N)
	{
		return FMath::Frac(FMath::Sin(N) * 43758.5453f);
	}
}

void UGlyphuxDrawData::Update(TConstArrayView<float> InQuads, TConstArrayView<float> InRecords, TConstArrayView<float> InTables, float InPxRange, bool bInAnimated)
{
	Quads = TArray<float>(InQuads.GetData(), InQuads.Num());
	PxRange = InPxRange;
	bAnimated = bInAnimated;
	WriteTexels(this, Records, InRecords, GlyphuxRecords::RecordsWidth, TEXT("GlyphuxRecords"));
	WriteTexels(this, Tables, InTables, GlyphuxRecords::TablesWidth, TEXT("GlyphuxTables"));
	BuildDrawQuads();
	bSurfaceBuilt = false;
	if (bWantsSurface)
	{
		BuildSurface();
	}
	BindTextures();
}

void UGlyphuxDrawData::SetBox(const FVector4f& InBox)
{
	Box = InBox;
	BindTextures();
}

UTexture2D* UGlyphuxDrawData::GetSurfaceQuads()
{
	bWantsSurface = true;
	BuildSurface();
	return SurfaceQuads;
}

UTexture2D* UGlyphuxDrawData::GetSurfaceGrid()
{
	bWantsSurface = true;
	BuildSurface();
	return SurfaceGrid;
}

void UGlyphuxDrawData::BuildDrawQuads()
{
	// Every quad, as the surface quads are laid out (three texels each), in drawing order with
	// its own pass: the world material reads a vertex's texture coordinates from here, because
	// a procedural mesh keeps its vertices' texture coordinates at half precision, too coarse
	// for the atlas. The vertices carry only exact numbers: their corner and the quad's index.
	TArray<float> Texels;
	Texels.Reserve(NumQuads() * 12);
	for (int32 I = 0; I < NumQuads(); ++I)
	{
		const float* Q = Quads.GetData() + I * GlyphuxRecords::QuadFloats;
		Texels.Append({Q[0], Q[1], Q[2], Q[3], Q[4], Q[5], Q[12], Q[13], Q[8], Q[9], Q[10], Q[11]});
	}
	WriteTexels(this, DrawQuads, Texels, 3 * GlyphuxRecords::SurfaceQuadsPerRow, TEXT("GlyphuxDrawQuads"));
}

const TArray<FGlyphuxSurfaceQuad>& UGlyphuxDrawData::GetSurfaceQuadList()
{
	bWantsSurface = true;
	BuildSurface();
	return SurfaceList;
}

void UGlyphuxDrawData::BuildSurface()
{
	if (bSurfaceBuilt)
	{
		return;
	}
	bSurfaceBuilt = true;
	// The quads a surface draws, one per glyph, all its passes at once: a monochrome glyph's
	// largest quad (its effects' if it has any, else its outline's, else its fill's; the quads
	// come in that order) and every color glyph. Three texels each: top-left and top-right
	// corners; bottom-left corner, record, pass; texture coordinates.
	TArray<float> Surface;
	TArray<FBox2f> Bounds;
	TSet<int32> Seen;
	SurfaceList.Reset();
	for (int32 I = 0; I < NumQuads(); ++I)
	{
		const float* Q = Quads.GetData() + I * GlyphuxRecords::QuadFloats;
		const int32 Record = FMath::RoundToInt(Q[12]);
		const bool bColor = FMath::RoundToInt(Q[13]) >= GlyphuxRecords::PassColor;
		bool bAlready = false;
		Seen.Add(bColor ? -1 - Record : Record, &bAlready);
		if (bAlready)
		{
			continue;
		}
		const float Pass = bColor ? static_cast<float>(GlyphuxRecords::PassColor) : static_cast<float>(GlyphuxRecords::PassAll);
		// Corners are top-left, top-right, bottom-left, bottom-right; a slanted quad is still a
		// parallelogram, so three of them place it.
		Surface.Append({Q[0], Q[1], Q[2], Q[3], Q[4], Q[5], Q[12], Pass, Q[8], Q[9], Q[10], Q[11]});
		SurfaceList.Add({FVector2f(Q[0], Q[1]), FVector2f(Q[2], Q[3]), FVector2f(Q[4], Q[5])});
		FBox2f B(ForceInit);
		for (int32 C = 0; C < 4; ++C)
		{
			B += FVector2f(Q[2 * C], Q[2 * C + 1]);
		}
		Bounds.Add(B);
	}
	WriteTexels(this, SurfaceQuads, Surface, 3 * GlyphuxRecords::SurfaceQuadsPerRow, TEXT("GlyphuxSurfaceQuads"));

	// The grid: texel 0 its origin, cell size and columns; texel 1 its rows; then a texel per
	// cell (where its list starts, how long it is); then the lists, four quad indices a texel.
	FBox2f All(ForceInit);
	for (const FBox2f& B : Bounds)
	{
		All += B;
	}
	const FVector2f Size = All.bIsValid ? All.GetSize() : FVector2f(1.0f, 1.0f);
	// Cells about as tall as the glyphs' quads, at most 64 by 64.
	float AverageHeight = 0.0f;
	for (const FBox2f& B : Bounds)
	{
		AverageHeight += B.GetSize().Y;
	}
	AverageHeight = Bounds.Num() > 0 ? AverageHeight / Bounds.Num() : 1.0f;
	const float Cell = FMath::Max3(AverageHeight, Size.X / 64.0f, Size.Y / 64.0f) + 1e-3f;
	const int32 Cols = FMath::Max(1, FMath::CeilToInt(Size.X / Cell));
	const int32 Rows = FMath::Max(1, FMath::CeilToInt(Size.Y / Cell));
	TArray<TArray<int32>> Cells;
	Cells.SetNum(Cols * Rows);
	for (int32 I = 0; I < Bounds.Num(); ++I)
	{
		const FVector2f Min = (Bounds[I].Min - All.Min) / Cell;
		const FVector2f Max = (Bounds[I].Max - All.Min) / Cell;
		for (int32 Y = FMath::Clamp(FMath::FloorToInt(Min.Y), 0, Rows - 1); Y <= FMath::Clamp(FMath::FloorToInt(Max.Y), 0, Rows - 1); ++Y)
		{
			for (int32 X = FMath::Clamp(FMath::FloorToInt(Min.X), 0, Cols - 1); X <= FMath::Clamp(FMath::FloorToInt(Max.X), 0, Cols - 1); ++X)
			{
				Cells[Y * Cols + X].Add(I);
			}
		}
	}
	TArray<float> Grid = {All.Min.X, All.Min.Y, Cell, static_cast<float>(Cols), static_cast<float>(Rows), static_cast<float>(Bounds.Num()), 0.0f, 0.0f};
	TArray<float> Lists;
	for (const TArray<int32>& C : Cells)
	{
		Grid.Append({static_cast<float>(Lists.Num()), static_cast<float>(C.Num()), 0.0f, 0.0f});
		for (int32 Index : C)
		{
			Lists.Add(static_cast<float>(Index));
		}
	}
	// The lists start at a whole texel after the cells.
	const float ListBase = static_cast<float>(Grid.Num() / 4);
	for (int32 C = 0; C < Cells.Num(); ++C)
	{
		Grid[8 + 4 * C] += 4.0f * ListBase;
	}
	Grid.Append(Lists);
	WriteTexels(this, SurfaceGrid, Grid, GlyphuxRecords::TablesWidth, TEXT("GlyphuxSurfaceGrid"));
	BindTextures();
}

int32 UGlyphuxDrawData::NumQuads() const
{
	return Quads.Num() / GlyphuxRecords::QuadFloats;
}

UMaterialInstanceDynamic* UGlyphuxDrawData::GetMaterial(UMaterialInterface* Base, bool bSurface)
{
	if (!Base)
	{
		return nullptr;
	}
	if (bSurface && !bWantsSurface)
	{
		bWantsSurface = true;
		BuildSurface();
	}
	for (UMaterialInstanceDynamic* Material : Materials)
	{
		if (Material && Material->Parent == Base)
		{
			return Material;
		}
	}
	UMaterialInstanceDynamic* Material = UMaterialInstanceDynamic::Create(Base, this);
	Materials.Add(Material);
	BindTo(Material);
	return Material;
}

void UGlyphuxDrawData::Bind(UMaterialInstanceDynamic* Target)
{
	if (!Target)
	{
		return;
	}
	if (!bWantsSurface)
	{
		bWantsSurface = true;
		BuildSurface();
	}
	Bound.RemoveAll([Target](const TWeakObjectPtr<UMaterialInstanceDynamic>& M) { return !M.IsValid() || M.Get() == Target; });
	Bound.Add(Target);
	BindTo(Target);
}

void UGlyphuxDrawData::BindTextures()
{
	for (UMaterialInstanceDynamic* Material : Materials)
	{
		BindTo(Material);
	}
	Bound.RemoveAll([](const TWeakObjectPtr<UMaterialInstanceDynamic>& M) { return !M.IsValid(); });
	for (const TWeakObjectPtr<UMaterialInstanceDynamic>& Material : Bound)
	{
		BindTo(Material.Get());
	}
}

void UGlyphuxDrawData::BindTo(UMaterialInstanceDynamic* Material) const
{
	if (!Material)
	{
		return;
	}
	// Parameters the material lacks are ignored.
	if (UGlyphuxSubsystem* Subsystem = UGlyphuxSubsystem::Get())
	{
		Material->SetTextureParameterValue(TEXT("GlyphuxAtlas"), Subsystem->GetAtlasTexture());
		Material->SetTextureParameterValue(TEXT("GlyphuxColorPages"), Subsystem->GetColorTexture());
	}
	if (Records)
	{
		Material->SetTextureParameterValue(TEXT("GlyphuxRecords"), Records);
	}
	if (Tables)
	{
		Material->SetTextureParameterValue(TEXT("GlyphuxTables"), Tables);
	}
	if (DrawQuads)
	{
		Material->SetTextureParameterValue(TEXT("GlyphuxDrawQuads"), DrawQuads);
	}
	if (bSurfaceBuilt && SurfaceQuads)
	{
		Material->SetTextureParameterValue(TEXT("GlyphuxSurfaceQuads"), SurfaceQuads);
	}
	if (bSurfaceBuilt && SurfaceGrid)
	{
		Material->SetTextureParameterValue(TEXT("GlyphuxSurfaceGrid"), SurfaceGrid);
	}
	Material->SetScalarParameterValue(TEXT("GlyphuxPxRange"), PxRange);
	Material->SetScalarParameterValue(TEXT("GlyphuxPages"), static_cast<float>(UGlyphuxSubsystem::MaxPages));
	Material->SetVectorParameterValue(TEXT("GlyphuxTextBox"), FLinearColor(Box.X, Box.Y, Box.Z, Box.W));
}

FVector2f UGlyphuxDrawData::Animation(int32 Index, float Time) const
{
	const float* Q = Quads.GetData() + Index * GlyphuxRecords::QuadFloats;
	// Q[14..19]: wave amplitude and frequency, shake amplitude and rate, phase and seed.
	FVector2f D(0.0f, 0.0f);
	if (Q[14] > 0.0f)
	{
		D.Y += Q[14] * FMath::Sin(Time * Q[15] * UE_TWO_PI + Q[18] * 0.05f);
	}
	if (Q[16] > 0.0f)
	{
		const float Step = FMath::FloorToFloat(Time * Q[17]) + Q[19] * 17.0f;
		D += Q[16] * FVector2f(Hash(Step) * 2.0f - 1.0f, Hash(Step + 7.31f) * 2.0f - 1.0f);
	}
	return D;
}
