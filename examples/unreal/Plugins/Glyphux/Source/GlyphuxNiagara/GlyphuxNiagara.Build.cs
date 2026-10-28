// Glyphux text in Niagara: a function library that hands a text component's glyphs to a
// Niagara system (one particle per glyph, or per fragment of a glyph), and the shard sets
// that cut glyphs into fragments. NIAGARA.md says how to build the emitters.

using UnrealBuildTool;

public class GlyphuxNiagara : ModuleRules
{
	public GlyphuxNiagara(ReadOnlyTargetRules Target) : base(Target)
	{
		PCHUsage = PCHUsageMode.UseExplicitOrSharedPCHs;
		PublicDependencyModuleNames.AddRange(new string[] { "Core", "CoreUObject", "Engine", "Glyphux", "Niagara" });
	}
}
