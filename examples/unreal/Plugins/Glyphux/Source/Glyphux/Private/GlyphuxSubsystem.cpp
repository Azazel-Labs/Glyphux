#include "GlyphuxSubsystem.h"

#include "Engine/Engine.h"
#include "Engine/FontFace.h"
#include "Engine/Texture2D.h"
#include "GlyphuxGpu.h"
#include "GlyphuxSettings.h"
#include "Materials/MaterialInterface.h"
#include "Misc/FileHelper.h"
#include "RenderingThread.h"
#include "Rivet/RivetGlyphux.h"
#include "TextureResource.h"

DEFINE_LOG_CATEGORY_STATIC(LogGlyphux, Log, All);

namespace
{
	/** A texture of MaxPages pages stacked vertically, RGBA8, sampled linearly. */
	UTexture2D* CreatePages(UObject* Outer, const TCHAR* Name)
	{
		UTexture2D* Texture = UTexture2D::CreateTransient(UGlyphuxSubsystem::PageSize,
			UGlyphuxSubsystem::PageSize * UGlyphuxSubsystem::MaxPages, PF_R8G8B8A8, FName(Name));
		if (!Texture)
		{
			return nullptr;
		}
		Texture->SRGB = false;
		Texture->Filter = TF_Bilinear;
		Texture->AddressX = TA_Clamp;
		Texture->AddressY = TA_Clamp;
		Texture->NeverStream = true;
		Texture->UpdateResource();
		return Texture;
	}
}

UGlyphuxSubsystem* UGlyphuxSubsystem::Get()
{
	return GEngine ? GEngine->GetEngineSubsystem<UGlyphuxSubsystem>() : nullptr;
}

void UGlyphuxSubsystem::Initialize(FSubsystemCollectionBase& Collection)
{
	Super::Initialize(Collection);
	Atlas = CreatePages(this, TEXT("GlyphuxAtlas"));
	ColorPages = CreatePages(this, TEXT("GlyphuxColorPages"));
	if (!Atlas || !ColorPages)
	{
		UE_LOG(LogGlyphux, Error, TEXT("Glyphux: cannot create the atlas textures; text will not draw"));
		return;
	}
	Gpu = MakeShared<FGlyphuxGpu, ESPMode::ThreadSafe>(PageSize, MaxPages);
	Rivet::Glyphux::Init(this);
	bStarted = true;
}

void UGlyphuxSubsystem::Deinitialize()
{
	bStarted = false;
	Rivet::Glyphux::Shutdown();
	Pending.Reset();
	// The render thread owns the GPU side from here: release it there, after what is queued.
	TSharedPtr<FGlyphuxGpu, ESPMode::ThreadSafe> Released = MoveTemp(Gpu);
	ENQUEUE_RENDER_COMMAND(GlyphuxRelease)([ToRelease = MoveTemp(Released)](FRHICommandListImmediate&) mutable { ToRelease.Reset(); });
	FlushRenderingCommands();
	Super::Deinitialize();
}

void UGlyphuxSubsystem::Tick(float DeltaTime)
{
	Rivet::Glyphux::Tick();
}

TStatId UGlyphuxSubsystem::GetStatId() const
{
	RETURN_QUICK_DECLARE_CYCLE_STAT(UGlyphuxSubsystem, STATGROUP_Tickables);
}

UMaterialInterface* UGlyphuxSubsystem::GetUIMaterial()
{
	if (!UIMaterial)
	{
		UIMaterial = GetDefault<UGlyphuxSettings>()->UIMaterial.LoadSynchronous();
		if (!UIMaterial && !bWarnedMaterials)
		{
			bWarnedMaterials = true;
			UE_LOG(LogGlyphux, Warning, TEXT("Glyphux: the UI material does not load (Project Settings > Plugins > Glyphux); create the materials with the plugin's Content/Python/glyphux_materials.py"));
		}
	}
	return UIMaterial;
}

UMaterialInterface* UGlyphuxSubsystem::GetWorldMaterial(bool bDepthTest)
{
	TObjectPtr<UMaterialInterface>& Material = WorldMaterials[bDepthTest ? 0 : 1];
	if (!Material)
	{
		const UGlyphuxSettings* Settings = GetDefault<UGlyphuxSettings>();
		Material = (bDepthTest ? Settings->WorldMaterial : Settings->WorldMaterialNoDepthTest).LoadSynchronous();
		if (!Material && !bWarnedMaterials)
		{
			bWarnedMaterials = true;
			UE_LOG(LogGlyphux, Warning, TEXT("Glyphux: the world materials do not load (Project Settings > Plugins > Glyphux); create the materials with the plugin's Content/Python/glyphux_materials.py"));
		}
	}
	return Material;
}

namespace
{
	UMaterialInterface* LoadMaterial(TObjectPtr<UMaterialInterface>& Loaded, const TSoftObjectPtr<UMaterialInterface>& Path, const TCHAR* What)
	{
		if (!Loaded)
		{
			Loaded = Path.LoadSynchronous();
			static TSet<FString> Warned;
			bool bWarned = false;
			Warned.Add(What, &bWarned);
			if (!Loaded && !bWarned)
			{
				UE_LOG(LogGlyphux, Warning, TEXT("Glyphux: the %s material (%s) does not load (Project Settings > Plugins > Glyphux); create the materials with the plugin's Content/Python/glyphux_materials.py"),
					What, *Path.ToString());
			}
		}
		return Loaded;
	}
}

UMaterialInterface* UGlyphuxSubsystem::GetGlyphMaterial()
{
	return LoadMaterial(GlyphMaterial, GetDefault<UGlyphuxSettings>()->GlyphMaterial, TEXT("glyph"));
}

UMaterialInterface* UGlyphuxSubsystem::GetSurfaceMaterial()
{
	return LoadMaterial(SurfaceMaterial, GetDefault<UGlyphuxSettings>()->SurfaceMaterial, TEXT("surface"));
}

FString UGlyphuxSubsystem::GetGlyphuxStats()
{
	return Rivet::Glyphux::Stats();
}

void UGlyphuxSubsystem::BeginBatch(uint32 Pages)
{
	Pending = MakeShared<FGlyphuxBatch, ESPMode::ThreadSafe>();
	Pending->Pages = Pages;
}

void UGlyphuxSubsystem::BatchInput(uint32 Binding, TConstArrayView<uint8> Bytes)
{
	if (Pending && Binding < GlyphuxKernels::NumBindings)
	{
		Pending->Inputs[Binding] = TArray<uint8>(Bytes.GetData(), Bytes.Num());
	}
}

void UGlyphuxSubsystem::BatchOutput(uint32 Binding, uint32 Size)
{
	if (Pending && Binding < GlyphuxKernels::NumBindings)
	{
		Pending->OutputSizes[Binding] = Size;
	}
}

void UGlyphuxSubsystem::BatchDispatch(uint32 Kernel, uint32 X, uint32 Y)
{
	if (Pending)
	{
		Pending->Dispatches.Add({static_cast<int32>(Kernel), X, Y});
	}
}

void UGlyphuxSubsystem::BatchRows(uint32 Layer, uint32 Start, uint32 End)
{
	if (Pending)
	{
		Pending->Rows.Add({Layer, Start, End});
	}
}

void UGlyphuxSubsystem::SubmitBatch()
{
	if (!Pending || !Gpu || !Atlas)
	{
		Pending.Reset();
		return;
	}
	FTextureResource* AtlasResource = Atlas->GetResource();
	ENQUEUE_RENDER_COMMAND(GlyphuxBatch)(
		[GpuRef = Gpu, Batch = MoveTemp(Pending), AtlasResource](FRHICommandListImmediate& RHICmdList)
		{
			FRHITexture* Target = AtlasResource ? AtlasResource->TextureRHI.GetReference() : nullptr;
			GpuRef->Run_RenderThread(RHICmdList, *Batch, Target);
		});
}

void UGlyphuxSubsystem::WriteColorRows(uint32 Page, uint32 X, uint32 Y, uint32 Width, uint32 Height, TConstArrayView<uint8> Bytes)
{
	if (!ColorPages || Width == 0 || Height == 0 || Page >= static_cast<uint32>(MaxPages)
		|| Bytes.Num() < static_cast<int32>(Width * Height * 4))
	{
		return;
	}
	// The texels must outlive the call: the render thread copies them later, then frees them.
	uint8* Data = static_cast<uint8*>(FMemory::Malloc(Bytes.Num()));
	FMemory::Memcpy(Data, Bytes.GetData(), Bytes.Num());
	FUpdateTextureRegion2D* Region = new FUpdateTextureRegion2D(X, Page * PageSize + Y, 0, 0, Width, Height);
	ColorPages->UpdateTextureRegions(0, 1, Region, Width * 4, 4, Data,
		[](uint8* SrcData, const FUpdateTextureRegion2D* Regions)
		{
			FMemory::Free(SrcData);
			delete Regions;
		});
}

FString UGlyphuxSubsystem::FontPath(UFontFace* Font)
{
	return Font ? Font->GetPathName() : FString();
}

TArray<uint8> UGlyphuxSubsystem::FontBytes(UFontFace* Font)
{
	if (!Font)
	{
		return {};
	}
	// Fonts loaded inline keep their data; the others are read from their file (in packaged
	// games, the .ufont next to the asset).
	const FFontFaceDataConstRef& Data = Font->GetFontFaceData();
	if (Data->HasData())
	{
		return Data->GetData();
	}
	TArray<uint8> Bytes;
	const FString File = Font->GetFontFilename();
	if (!File.IsEmpty() && !FFileHelper::LoadFileToArray(Bytes, *File))
	{
		UE_LOG(LogGlyphux, Warning, TEXT("Glyphux: cannot read %s, the file of %s"), *File, *Font->GetPathName());
	}
	return Bytes;
}

UFontFace* UGlyphuxSubsystem::LoadFont(const FString& Path)
{
	return LoadObject<UFontFace>(nullptr, *Path);
}
