using UnrealBuildTool;

public class GlyphuxDemoTarget : TargetRules
{
	public GlyphuxDemoTarget(TargetInfo Target) : base(Target)
	{
		Type = TargetType.Game;
		DefaultBuildSettings = BuildSettingsVersion.Latest;
		IncludeOrderVersion = EngineIncludeOrderVersion.Latest;
		ExtraModuleNames.Add("GlyphuxDemo");
	}
}
