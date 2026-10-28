// Runs the glyph atlas' batches on the GPU, in Unreal's render graph: the compute kernels of
// glyphux-gpu (translated from their WGSL to HLSL when the plugin's Rust is built) write the
// fields into a texture array of pages, and the rows they wrote are copied into the atlas
// texture the materials sample (the pages stacked vertically, a plain 2D texture).

#pragma once

#include "CoreMinimal.h"
#include "Generated/GlyphuxKernels.h"
#include "RenderGraphResources.h"

/** One batch, as Rust describes it (see the plugin's rust/src/server.rs, Server::flush). */
struct FGlyphuxBatch
{
	/** The input buffers' contents, by binding (empty: not an input of this batch). */
	TArray<uint8> Inputs[GlyphuxKernels::NumBindings];
	/** The output buffers' sizes in bytes, by binding (0: not an output of this batch). */
	uint32 OutputSizes[GlyphuxKernels::NumBindings] = {};

	struct FDispatch
	{
		int32 Kernel;
		uint32 X;
		uint32 Y;
	};
	/** The kernels to run, in order, the store kernel last. */
	TArray<FDispatch> Dispatches;

	struct FRows
	{
		uint32 Layer;
		uint32 Start;
		uint32 End;
	};
	/** The texel rows the batch writes, per page. */
	TArray<FRows> Rows;

	/** Pages the atlas has. */
	uint32 Pages = 0;
};

/** The GPU side of the atlas. Created on the game thread, used and deleted on the render thread. */
class GLYPHUXSHADERS_API FGlyphuxGpu
{
public:
	FGlyphuxGpu(int32 InPageSize, int32 InMaxPages);

	/** Runs Batch and copies the rows it wrote into Atlas: MaxPages pages stacked vertically, PF_R8G8B8A8. */
	void Run_RenderThread(FRHICommandListImmediate& RHICmdList, const FGlyphuxBatch& Batch, FRHITexture* Atlas);

private:
	/** The pages the kernels write (a texture array, kept between batches). */
	TRefCountPtr<IPooledRenderTarget> Pages;
	int32 PageSize;
	int32 MaxPages;
};
