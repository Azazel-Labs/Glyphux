// The Glyphux plugin's main module: the text classes (their C++ parents, the subsystem, the
// drawing) and the Rust library that holds the rest. The classes' Rust is in ../../rust;
// building it (cargo, below) also writes the generated C++ (Public/Ferreal, Private/Ferreal,
// Private/Generated), the kernels' shader classes (../GlyphuxShaders), the shaders
// (../../Shaders/Private/Generated) and Ferreal's exporter (../GlyphuxExporter). The "Rust"
// region is Ferreal's template (its examples/UnrealGame FerrealSample.Build.cs), with rustup
// adding the build's target (AddRustTarget).
//
// Building needs Rust (https://rustup.rs); cargo fetches Ferreal from its repository (see the
// plugin's README). FERREAL_SKIP_CARGO=1 links the library already built instead.

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
		PrivateDependencyModuleNames.AddRange(new string[] { "RenderCore", "RHI", "InputCore", "GlyphuxShaders" });

		BuildRust(Target, Path.Combine(ModuleDirectory, "..", "..", "rust"), "glyphux_unreal", "unreal");
	}

	#region Rust

	/// <summary>
	/// Builds the Rust crate at CrateDir (cargo build, which runs its build.rs and so writes
	/// the generated C++) and links its static library, named LibraryName (the crate's name
	/// with underscores). Profile is the cargo profile: "unreal" is release with unwinding,
	/// declared in the workspace's Cargo.toml. FERREAL_SKIP_CARGO=1 in the environment links the
	/// library already built (for machines without Rust, with the libraries committed).
	/// The engine classes the crate names in ferreal::engine! are bound from the reflection
	/// data the Ferreal exporter writes (see EngineMetadataPath): the crate's build writes the
	/// exporter beside this module, and Unreal Build Tool builds it with the project.
	/// </summary>
	private void BuildRust(ReadOnlyTargetRules Target, string CrateDir, string LibraryName, string Profile)
	{
		CrateDir = ResolveLinks(Path.GetFullPath(CrateDir));
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

		// The engine's reflection data, for the classes ferreal::engine! names: what Ferreal's
		// exporter wrote during the last build (Unreal Header Tool runs after this file). A new
		// export makes the next build run this file, and cargo, again; so does the first,
		// through the file it will write (a dependency that does not exist runs this file in
		// every build until it does).
		string Export = EngineMetadataPath(Target);
		if (Export != null)
		{
			ExternalDependencies.Add(Export);
		}
		string Metadata = Export != null && File.Exists(Export) ? Export : null;
		// Unreal Build Tool caches what it learns from this file: these make it run the file
		// (and so cargo) again when the Rust changes.
		ExternalDependencies.Add(Path.Combine(CrateDir, "Cargo.toml"));
		ExternalDependencies.Add(Path.Combine(CrateDir, "build.rs"));
		foreach (string Source in Directory.EnumerateFiles(Path.Combine(CrateDir, "src"), "*.rs", SearchOption.AllDirectories))
		{
			ExternalDependencies.Add(Source);
		}

		if (Environment.GetEnvironmentVariable("FERREAL_SKIP_CARGO") != "1")
		{
			AddRustTarget(CrateDir, Triple);
			string Arguments = string.Format(
				"build --manifest-path \"{0}\" --lib --profile {1} --target {2} --target-dir \"{3}\"",
				Path.Combine(CrateDir, "Cargo.toml"), Profile, Triple, TargetDir);
			CargoResult Result = RunCargo(CrateDir, Arguments, Metadata);
			bool bBuilt = Result == CargoResult.Built;
			string Failed = ReportFailure(!bBuilt);
			string Pending = LeavePending(Target, bBuilt ? null : new string[] { CrateDir, Arguments, Failed });
			ForgetGenerated();
			AddEngineModules();
			if (!bBuilt)
			{
				// The crate may need reflection data this build has yet to export: engine classes
				// it names for the first time, or functions new to the C++. Ferreal's exporter
				// builds it again once Unreal Header Tool has exported the data, later in this
				// build and before the C++ compiles, and lifts the failure if that succeeds.
				// Unreal Header Tool runs when headers changed (always, for new data); when it
				// does not, the failure stands.
				Console.WriteLine(Pending == null
					? "Rust: cargo failed (its errors are above)."
					: Result == CargoResult.NeedsMetadata
						? "Rust: the reflection data ferreal::engine! needs is not exported yet: the crate is built once Unreal Header Tool has exported it, later in this build."
						: "Rust: cargo failed (its errors are above). If Unreal Header Tool runs in this build, the crate is built again with the reflection data it exports.");
				PublicAdditionalLibraries.Add(Library);
				return;
			}
		}
		else
		{
			AddEngineModules();
		}
		if (!File.Exists(Library))
		{
			throw new BuildException("Rust: " + Library + " was not built");
		}
		PublicAdditionalLibraries.Add(Library);
		// Cargo's dep-info, named after the library (glyphux_unreal.d, libglyphux_unreal.d).
		AddCargoDependencies(Path.ChangeExtension(Library, ".d"), TargetDir, CrateDir);
	}

	/// <summary>
	/// Depends on the modules of the engine classes, structs and enums the crate uses (whose
	/// headers the generated C++ includes), which ferreal-build lists in
	/// Private/Ferreal/FerrealModules.txt from the reflection data.
	/// </summary>
	private void AddEngineModules()
	{
		string Modules = Path.Combine(ModuleDirectory, "Private", "Ferreal", "FerrealModules.txt");
		if (!File.Exists(Modules))
		{
			return;
		}
		foreach (string Line in File.ReadAllLines(Modules))
		{
			string Module = Line.Trim();
			if (Module.Length > 0 && !Module.StartsWith("//") && Module != Name && !PublicDependencyModuleNames.Contains(Module))
			{
				PublicDependencyModuleNames.Add(Module);
			}
		}
	}

	/// <summary>
	/// Makes Unreal Build Tool run this file (and so cargo) again when anything the library was
	/// built from changes: the files cargo's dep-info lists (the crate's and its path
	/// dependencies' sources, ferreal-build's among them, which generates the C++), and the
	/// workspace's Cargo.lock (other dependencies' versions).
	/// </summary>
	private void AddCargoDependencies(string DepInfo, string TargetDir, string CrateDir)
	{
		for (DirectoryInfo Dir = new DirectoryInfo(CrateDir); Dir != null; Dir = Dir.Parent)
		{
			string Lock = Path.Combine(Dir.FullName, "Cargo.lock");
			if (File.Exists(Lock))
			{
				ExternalDependencies.Add(Lock);
				break;
			}
		}
		if (!File.Exists(DepInfo))
		{
			return;
		}
		// `target: a b c`, a space in a path escaped as `\ `.
		string Text = File.ReadAllText(DepInfo);
		int Colon = Text.IndexOf(": ", StringComparison.Ordinal);
		if (Colon < 0)
		{
			return;
		}
		string Rest = Text.Substring(Colon + 2).Replace("\\ ", "\u0001");
		foreach (string Part in Rest.Split(new char[] { ' ', '\n', '\r' }, StringSplitOptions.RemoveEmptyEntries))
		{
			string Dependency = Part.Replace('\u0001', ' ');
			// What cargo generates under its target folder changes with every build.
			if (!Dependency.StartsWith(TargetDir, StringComparison.OrdinalIgnoreCase) && File.Exists(Dependency))
			{
				ExternalDependencies.Add(Dependency);
			}
		}
	}

	/// <summary>
	/// When cargo failed, writes a source file that fails the build (Private/Ferreal/
	/// FerrealCargoFailed.cpp) rather than stopping it here: Unreal Header Tool runs first, so
	/// the reflection data is exported all the same, and Ferreal's exporter builds the crate
	/// again with it (see LeavePending), emptying the file if that succeeds. Deletes the file
	/// once cargo succeeds. Returns its path.
	/// </summary>
	private string ReportFailure(bool bFailed)
	{
		string Failed = Path.Combine(ModuleDirectory, "Private", "Ferreal", "FerrealCargoFailed.cpp");
		if (bFailed)
		{
			Directory.CreateDirectory(Path.GetDirectoryName(Failed));
			File.WriteAllText(Failed, "// Written by the module's Build.cs: cargo failed. Deleted once it succeeds.\n#error \"Rust: cargo failed (its errors are above). If the Rust calls functions new to the C++, build again: this build exported them.\"\n");
		}
		else if (File.Exists(Failed))
		{
			File.Delete(Failed);
		}
		return Failed;
	}

	/// <summary>
	/// Leaves the cargo build that failed (Build: the crate's directory, cargo's arguments, the
	/// file failing the build) to Ferreal's exporter, which runs it again with the reflection
	/// data it exports later in this build: <Project>/Intermediate/Ferreal/Pending/<Module>.txt.
	/// Removes it when Build is null. Returns its path, or null without a project.
	/// The exporter's build rewrites the generated sources after Unreal Build Tool has looked at
	/// them: they are marked changed now, so that what it compiles is what that build wrote.
	/// </summary>
	private string LeavePending(ReadOnlyTargetRules Target, string[] Build)
	{
		if (Target.ProjectFile == null)
		{
			return null;
		}
		string Pending = Path.Combine(Target.ProjectFile.Directory.FullName, "Intermediate", "Ferreal", "Pending", Name + ".txt");
		if (Build == null)
		{
			if (File.Exists(Pending))
			{
				File.Delete(Pending);
			}
			return null;
		}
		Directory.CreateDirectory(Path.GetDirectoryName(Pending));
		File.WriteAllLines(Pending, Build);
		string Generated = Path.Combine(ModuleDirectory, "Private", "Ferreal");
		if (Directory.Exists(Generated))
		{
			foreach (string Source in Directory.EnumerateFiles(Generated))
			{
				File.SetLastWriteTimeUtc(Source, DateTime.UtcNow);
			}
		}
		return Pending;
	}

	/// <summary>
	/// Makes Unreal Build Tool read the module's folders again, the generated C++ among them
	/// (Public/Ferreal, Private/Ferreal): cargo writes it while this file runs, after the tool
	/// has cached what it found there (looking for this file), and it would then compile the old
	/// files, or miss new ones and new folders (Unreal Header Tool with them).
	/// </summary>
	private void ForgetGenerated()
	{
		UnrealBuildBase.DirectoryItem.ResetCachedInfo(ModuleDirectory);
	}

	/// <summary>
	/// Dir with the links among it and its parents resolved: in a plugin linked into the
	/// project's Plugins folder (a symbolic link, a junction), cargo would otherwise find the
	/// crate's workspace by walking up the link's path, not the crate's.
	/// </summary>
	private static string ResolveLinks(string Dir)
	{
		string Below = null;
		for (DirectoryInfo Info = new DirectoryInfo(Dir); Info != null; Info = Info.Parent)
		{
			FileSystemInfo Link = Info.LinkTarget != null ? Info.ResolveLinkTarget(true) : null;
			if (Link != null)
			{
				return ResolveLinks(Below == null ? Link.FullName : Path.Combine(Link.FullName, Below));
			}
			Below = Below == null ? Info.Name : Path.Combine(Info.Name, Below);
		}
		return Dir;
	}

	/// <summary>
	/// Where the Ferreal exporter writes the reflection data of the project's build:
	/// <Project>/Intermediate/Ferreal/FerrealEngine.json; FERREAL_ENGINE_METADATA, if set,
	/// instead. Null for a target without a project.
	/// </summary>
	private static string EngineMetadataPath(ReadOnlyTargetRules Target)
	{
		string Set = Environment.GetEnvironmentVariable("FERREAL_ENGINE_METADATA");
		if (!string.IsNullOrEmpty(Set))
		{
			return Path.GetFullPath(Set);
		}
		if (Target.ProjectFile == null)
		{
			return null;
		}
		return Path.Combine(Target.ProjectFile.Directory.FullName, "Intermediate", "Ferreal", "FerrealEngine.json");
	}

	private enum CargoResult
	{
		Built,
		/// <summary>The crate needs reflection data Unreal has not exported yet.</summary>
		NeedsMetadata,
		Failed,
	}

	/// <summary>
	/// Has rustup install the target Triple (and the crate's toolchain, rust-toolchain.toml)
	/// if they are missing: nothing to do once they are there. Without rustup (a Rust
	/// installed otherwise), cargo reports a missing target itself.
	/// </summary>
	private static void AddRustTarget(string WorkingDirectory, string Triple)
	{
		ProcessStartInfo Start = new ProcessStartInfo("rustup", "target add " + Triple)
		{
			WorkingDirectory = WorkingDirectory,
			UseShellExecute = false,
			RedirectStandardOutput = true,
			RedirectStandardError = true,
			CreateNoWindow = true,
		};
		try
		{
			using Process Rustup = Process.Start(Start);
			System.Threading.Tasks.Task<string> Output = Rustup.StandardOutput.ReadToEndAsync();
			string Errors = Rustup.StandardError.ReadToEnd();
			Rustup.WaitForExit();
			if (Rustup.ExitCode != 0)
			{
				Console.WriteLine("Rust: rustup target add " + Triple + " failed (cargo will say what is missing):\n" + Output.Result + Errors);
			}
		}
		catch (Exception)
		{
			// No rustup.
		}
	}

	/// <summary>
	/// Runs cargo with Arguments (and the engine's reflection data at Metadata, if any),
	/// writing its errors to the build's output when it fails.
	/// </summary>
	private static CargoResult RunCargo(string WorkingDirectory, string Arguments, string Metadata)
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
			Start.Environment["FERREAL_ENGINE_METADATA"] = Metadata;
		}
		Console.WriteLine("Rust: cargo " + Arguments);
		Process Cargo;
		try
		{
			Cargo = Process.Start(Start);
		}
		catch (Exception E)
		{
			throw new BuildException("Rust: cannot run cargo (install Rust from https://rustup.rs, or set FERREAL_SKIP_CARGO=1 to link a library already built): " + E.Message);
		}
		// Read both streams at once: cargo can fill one while we wait on the other.
		System.Threading.Tasks.Task<string> Output = Cargo.StandardOutput.ReadToEndAsync();
		string Errors = Cargo.StandardError.ReadToEnd();
		Cargo.WaitForExit();
		if (Cargo.ExitCode == 0)
		{
			return CargoResult.Built;
		}
		if (Errors.Contains("FERREAL_NEEDS_ENGINE_METADATA"))
		{
			Console.WriteLine(Errors);
			return CargoResult.NeedsMetadata;
		}
		Console.WriteLine("Rust: cargo failed:\n" + Output.Result + Errors);
		return CargoResult.Failed;
	}

	#endregion
}
