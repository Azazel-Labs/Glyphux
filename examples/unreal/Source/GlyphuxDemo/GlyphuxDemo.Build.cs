using UnrealBuildTool;

public class GlyphuxDemo : ModuleRules
{
	public GlyphuxDemo(ReadOnlyTargetRules Target) : base(Target)
	{
		PCHUsage = PCHUsageMode.UseExplicitOrSharedPCHs;
		PublicDependencyModuleNames.AddRange(new string[] { "Core", "CoreUObject", "Engine", "Glyphux", "GlyphuxNiagara", "Niagara" });
	}
}
