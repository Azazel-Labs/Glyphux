using UnrealBuildTool;

public class GlyphuxDemoEditorTarget : TargetRules
{
	public GlyphuxDemoEditorTarget(TargetInfo Target) : base(Target)
	{
		Type = TargetType.Editor;
		DefaultBuildSettings = BuildSettingsVersion.Latest;
		IncludeOrderVersion = EngineIncludeOrderVersion.Latest;
		ExtraModuleNames.Add("GlyphuxDemo");
	}
}
