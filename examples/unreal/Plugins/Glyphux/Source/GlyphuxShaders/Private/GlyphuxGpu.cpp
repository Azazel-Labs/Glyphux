#include "GlyphuxGpu.h"

#include "RenderGraphBuilder.h"
#include "RenderGraphUtils.h"
#include "RenderTargetPool.h"

DEFINE_LOG_CATEGORY_STATIC(LogGlyphuxGpu, Log, All);

FGlyphuxGpu::FGlyphuxGpu(int32 InPageSize, int32 InMaxPages)
	: PageSize(InPageSize)
	, MaxPages(InMaxPages)
{
}

void FGlyphuxGpu::Run_RenderThread(FRHICommandListImmediate& RHICmdList, const FGlyphuxBatch& Batch, FRHITexture* Atlas)
{
	check(IsInRenderingThread());
	FRDGBuilder GraphBuilder(RHICmdList, RDG_EVENT_NAME("Glyphux"));

	// The pages: allocated once at their largest (the atlas never has more), kept between batches.
	const bool bNew = !Pages.IsValid();
	FRDGTextureRef PagesTexture = bNew
		? GraphBuilder.CreateTexture(
			FRDGTextureDesc::Create2DArray(FIntPoint(PageSize, PageSize), PF_R8G8B8A8, FClearValueBinding::Transparent,
				TexCreate_ShaderResource | TexCreate_UAV, MaxPages),
			TEXT("Glyphux.Pages"))
		: GraphBuilder.RegisterExternalTexture(Pages);

	// The batch's buffers, at exactly their size: the kernels take the lengths of the work
	// list and of the corners from their bindings.
	GlyphuxKernels::FBindings Bindings;
	for (int32 Binding = 0; Binding < GlyphuxKernels::NumBindings; ++Binding)
	{
		const TArray<uint8>& Input = Batch.Inputs[Binding];
		const uint32 Size = Input.Num() > 0 ? static_cast<uint32>(Input.Num()) : Batch.OutputSizes[Binding];
		if (Size == 0)
		{
			continue;
		}
		if (Size % 4 != 0)
		{
			UE_LOG(LogGlyphuxGpu, Error, TEXT("%s: %u bytes, not a whole number of words; batch skipped"), GlyphuxKernels::BindingName(Binding), Size);
			return;
		}
		FRDGBufferRef Buffer = GraphBuilder.CreateBuffer(FRDGBufferDesc::CreateByteAddressDesc(Size), GlyphuxKernels::BindingName(Binding));
		if (Input.Num() > 0)
		{
			GraphBuilder.QueueBufferUpload(Buffer, Input.GetData(), Input.Num());
		}
		if (GlyphuxKernels::IsOutput(Binding))
		{
			Bindings.UAV[Binding] = GraphBuilder.CreateUAV(FRDGBufferUAVDesc(Buffer));
			// The error correction flags must start cleared.
			AddClearUAVPass(GraphBuilder, Bindings.UAV[Binding], 0u);
		}
		else
		{
			Bindings.SRV[Binding] = GraphBuilder.CreateSRV(FRDGBufferSRVDesc(Buffer));
		}
	}
	Bindings.Atlas = GraphBuilder.CreateUAV(FRDGTextureUAVDesc(PagesTexture));

	for (const FGlyphuxBatch::FDispatch& D : Batch.Dispatches)
	{
		if (!GlyphuxKernels::AddPass(GraphBuilder, D.Kernel, Bindings, FIntVector(D.X, D.Y, 1)))
		{
			UE_LOG(LogGlyphuxGpu, Error, TEXT("kernel %d: unknown, or a resource it binds is missing"), D.Kernel);
		}
	}

	// The rows written, into the atlas the materials sample.
	if (Atlas)
	{
		FRDGTextureRef AtlasTexture = GraphBuilder.RegisterExternalTexture(CreateRenderTarget(Atlas, TEXT("Glyphux.Atlas")));
		for (const FGlyphuxBatch::FRows& R : Batch.Rows)
		{
			if (R.End <= R.Start || static_cast<int32>(R.Layer) >= MaxPages)
			{
				continue;
			}
			FRHICopyTextureInfo Copy;
			Copy.Size = FIntVector(PageSize, static_cast<int32>(R.End - R.Start), 1);
			Copy.SourcePosition = FIntVector(0, static_cast<int32>(R.Start), 0);
			Copy.SourceSliceIndex = R.Layer;
			Copy.DestPosition = FIntVector(0, static_cast<int32>(R.Layer) * PageSize + static_cast<int32>(R.Start), 0);
			Copy.NumSlices = 1;
			AddCopyTexturePass(GraphBuilder, PagesTexture, AtlasTexture, Copy);
		}
	}

	if (bNew)
	{
		GraphBuilder.QueueTextureExtraction(PagesTexture, &Pages);
	}
	GraphBuilder.Execute();
}
