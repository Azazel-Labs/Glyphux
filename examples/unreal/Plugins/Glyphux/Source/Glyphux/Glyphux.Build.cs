// The Glyphux plugin's main module: the text classes (their C++ parents, the subsystem, the
// drawing) and the Rust library that holds the rest. The classes' Rust is in ../../rust;
// building it (cargo, below) also writes the generated C++ (Public/Rivet, Private/Rivet,
// Private/Generated), the kernels' shader classes (../GlyphuxShaders) and the shaders
// (../../Shaders/Private/Generated).
//
// Building needs Rust (https://rustup.rs) and a clone of Rivet next to the Glyphux repository
// (see the plugin's README). RIVET_SKIP_CARGO=1 links the library already built instead.

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using UnrealBuildTool;

public class Glyphux : ModuleRules
{
	public Glyphux(ReadOnlyTargetRules Target) : base(Target)
	{
		PCHUsage = PCHUsageMode.UseExplicitOrSharedPCHs;
		PublicDependencyModuleNames.AddRange(new string[]
		{
			"Core", "CoreUObject", "Engine", "UMG", "Slate", "SlateCore", "ProceduralMeshComponent", "DeveloperSettings",
		});
		PrivateDependencyModuleNames.AddRange(new string[] { "RenderCore", "RHI", "GlyphuxShaders" });

		BuildRust(Target, Path.Combine(ModuleDirectory, "..", "..", "rust"), "glyphux_unreal", "unreal");
	}

	#region Rust

	/// <summary>
	/// Builds the Rust crate at CrateDir (cargo build, which runs its build.rs and so writes
	/// the generated C++) and links its static library, named LibraryName (the crate's name
	/// with underscores). Profile is the cargo profile: "unreal" is release with unwinding,
	/// declared in the workspace's Cargo.toml. RIVET_SKIP_CARGO=1 in the environment links the
	/// library already built (for machines without Rust, with the libraries committed).
	/// The engine classes the crate names in rivet::engine! are bound from the reflection
	/// data of the Rivet plugin's exporter (see FindEngineMetadata).
	/// </summary>
	private void BuildRust(ReadOnlyTargetRules Target, string CrateDir, string LibraryName, string Profile)
	{
		CrateDir = Path.GetFullPath(CrateDir);
		string Triple;
		string FileName;
		if (Target.Platform == UnrealTargetPlatform.Win64)
		{
			Triple = "x86_64-pc-windows-msvc";
			FileName = LibraryName + ".lib";
			// What the Rust standard library links (rustc --print native-static-libs).
			PublicSystemLibraries.AddRange(new string[] { "kernel32.lib", "advapi32.lib", "ntdll.lib", "userenv.lib", "ws2_32.lib", "bcrypt.lib", "dbghelp.lib" });
		}
		else if (Target.Platform == UnrealTargetPlatform.Linux || Target.Platform == UnrealTargetPlatform.LinuxArm64)
		{
			Triple = Target.Platform == UnrealTargetPlatform.Linux ? "x86_64-unknown-linux-gnu" : "aarch64-unknown-linux-gnu";
			FileName = "lib" + LibraryName + ".a";
			PublicSystemLibraries.AddRange(new string[] { "gcc_s", "util", "rt", "pthread", "m", "dl" });
		}
		else if (Target.Platform == UnrealTargetPlatform.Mac)
		{
			Triple = "aarch64-apple-darwin";
			FileName = "lib" + LibraryName + ".a";
			PublicSystemLibraries.AddRange(new string[] { "c", "m" });
		}
		else
		{
			throw new BuildException("Rust: no Rust target is set up for " + Target.Platform + " (see BuildRust in " + ModuleDirectory + ")");
		}

		string TargetDir = Path.Combine(CrateDir, "target", "unreal");
		string Library = Path.Combine(TargetDir, Triple, Profile, FileName);

		// The engine's reflection data, for the classes rivet::engine! names: what the Rivet
		// plugin's exporter wrote during the last build (Unreal Header Tool runs after this
		// file). A new export makes the next build run this file, and cargo, again.
		string Metadata = FindEngineMetadata(Target);
		if (Metadata != null)
		{
			ExternalDependencies.Add(Metadata);
		}
		if (Environment.GetEnvironmentVariable("RIVET_SKIP_CARGO") != "1")
		{
			bool bNeedsMetadata = RunCargo(CrateDir, string.Format(
				"build --manifest-path \"{0}\" --lib --profile {1} --target {2} --target-dir \"{3}\"",
				Path.Combine(CrateDir, "Cargo.toml"), Profile, Triple, TargetDir), Metadata);
			if (bNeedsMetadata)
			{
				// The first build with new engine classes: this build exports their reflection
				// data, the next one generates their bindings and links the Rust.
				Console.WriteLine("Rust: the reflection data rivet::engine! needs is not exported yet; this build exports it (the Rivet plugin). Build again to generate the bindings and link the Rust.");
				if (!File.Exists(Library))
				{
					return;
				}
			}
		}
		if (!File.Exists(Library))
		{
			throw new BuildException("Rust: " + Library + " was not built");
		}
		PublicAdditionalLibraries.Add(Library);

		// Unreal Build Tool caches what it learns from this file: these make it run the file
		// (and so cargo) again when the Rust changes.
		ExternalDependencies.Add(Path.Combine(CrateDir, "Cargo.toml"));
		ExternalDependencies.Add(Path.Combine(CrateDir, "build.rs"));
		foreach (string Source in Directory.EnumerateFiles(Path.Combine(CrateDir, "src"), "*.rs", SearchOption.AllDirectories))
		{
			ExternalDependencies.Add(Source);
		}
	}

	/// <summary>
	/// The newest RivetEngine.json the Rivet plugin's exporter wrote (in its RivetMetadata
	/// module's generated folder), in the project's plugins; RIVET_ENGINE_METADATA, if set,
	/// instead. Null without either.
	/// </summary>
	private static string FindEngineMetadata(ReadOnlyTargetRules Target)
	{
		string Set = Environment.GetEnvironmentVariable("RIVET_ENGINE_METADATA");
		if (!string.IsNullOrEmpty(Set))
		{
			return File.Exists(Set) ? Path.GetFullPath(Set) : null;
		}
		if (Target.ProjectFile == null)
		{
			return null;
		}
		string Plugins = Path.Combine(Target.ProjectFile.Directory.FullName, "Plugins");
		if (!Directory.Exists(Plugins))
		{
			return null;
		}
		string Newest = null;
		DateTime NewestTime = DateTime.MinValue;
		foreach (string Descriptor in Directory.EnumerateFiles(Plugins, "Rivet.uplugin", SearchOption.AllDirectories))
		{
			string Build = Path.Combine(Path.GetDirectoryName(Descriptor), "Intermediate", "Build");
			if (!Directory.Exists(Build))
			{
				continue;
			}
			foreach (string Json in Directory.EnumerateFiles(Build, "RivetEngine.json", SearchOption.AllDirectories))
			{
				DateTime Time = File.GetLastWriteTimeUtc(Json);
				if (Json.Contains("RivetMetadata") && Time > NewestTime)
				{
					Newest = Json;
					NewestTime = Time;
				}
			}
		}
		return Newest;
	}

	/// <summary>
	/// Runs cargo with Arguments (and the engine's reflection data at Metadata, if any).
	/// Returns true when the build needs reflection data Unreal has not exported yet;
	/// throws on other failures.
	/// </summary>
	private static bool RunCargo(string WorkingDirectory, string Arguments, string Metadata)
	{
		ProcessStartInfo Start = new ProcessStartInfo("cargo", Arguments)
		{
			WorkingDirectory = WorkingDirectory,
			UseShellExecute = false,
			RedirectStandardOutput = true,
			RedirectStandardError = true,
			CreateNoWindow = true,
		};
		if (Metadata != null)
		{
			Start.Environment["RIVET_ENGINE_METADATA"] = Metadata;
		}
		Console.WriteLine("Rust: cargo " + Arguments);
		Process Cargo;
		try
		{
			Cargo = Process.Start(Start);
		}
		catch (Exception E)
		{
			throw new BuildException("Rust: cannot run cargo (install Rust from https://rustup.rs, or set RIVET_SKIP_CARGO=1 to link a library already built): " + E.Message);
		}
		// Read both streams at once: cargo can fill one while we wait on the other.
		System.Threading.Tasks.Task<string> Output = Cargo.StandardOutput.ReadToEndAsync();
		string Errors = Cargo.StandardError.ReadToEnd();
		Cargo.WaitForExit();
		if (Cargo.ExitCode == 0)
		{
			return false;
		}
		if (Errors.Contains("RIVET_NEEDS_ENGINE_METADATA"))
		{
			Console.WriteLine(Errors);
			return true;
		}
		throw new BuildException("Rust: cargo failed:\n" + Output.Result + Errors);
	}

	#endregion
}
