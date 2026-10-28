// The Glyphux plugin's early module: it registers the plugin's shader folder and the global
// shaders of glyphux-gpu's kernels (which must exist before the global shader map is
// compiled, so this module loads at PostConfigInit), and runs the kernels in the render
// graph. It has no UObjects; the Glyphux module has those.

using UnrealBuildTool;

public class GlyphuxShaders : ModuleRules
{
	public GlyphuxShaders(ReadOnlyTargetRules Target) : base(Target)
	{
		PCHUsage = PCHUsageMode.UseExplicitOrSharedPCHs;
		PublicDependencyModuleNames.AddRange(new string[] { "Core", "RenderCore", "RHI" });
		PrivateDependencyModuleNames.AddRange(new string[] { "CoreUObject", "Projects" });
	}
}
