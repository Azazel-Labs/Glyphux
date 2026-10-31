// Ferreal's exporter for Unreal Header Tool: writes the reflection data of every class, struct
// and enum UHT parsed (the engine's, the project's, its plugins') into one JSON file, which
// ferreal-build reads to generate typed Rust bindings of the engine classes a crate names.
//
// It writes what calling a function through reflection needs, and nothing about memory
// layout: the classes with their path and parent, their reflected functions with flags and
// parameters (each parameter's kind, the engine property class that will hold it, its
// flags), the structs with their reflected fields, and the enums with their values.
// ferreal-build lays out each call's parameters as a C++ struct, and the generated code checks
// that struct against the engine's own reflection when it first calls the function.
//
// ferreal-build writes this file, with a .ubtplugin.csproj and ExporterModule.cs, into each
// Rust plugin (Source/<Module>Exporter), and Unreal Build Tool builds it with the project.
// Each copy is named after its module (UHT takes one exporter per name); the first to run in
// a build exports for all.
//
// Output: <Project>/Intermediate/Ferreal/FerrealEngine.json. Then it builds the Rust crates
// whose module's Build.cs could not (it runs before this export, and their crate needed data
// it did not have yet): their libraries and C++ are then ready before the C++ is compiled, and
// a single build of a fresh checkout, or of a crate naming new engine classes, succeeds.

using System.Diagnostics;

using System.Text;
using System.Text.Encodings.Web;
using System.Text.Json;
using EpicGames.Core;
using EpicGames.UHT.Tables;
using EpicGames.UHT.Types;
using EpicGames.UHT.Utils;

namespace Ferreal;

[UnrealHeaderTool]
public static class FerrealExporter
{
	/// <summary>The format ferreal-build expects (ferreal-build/src/engine.rs, FORMAT_VERSION).</summary>
	private const int FormatVersion = 1;

	[UhtExporter(
		Name = "Ferreal." + ExporterModule.Name,
		Description = "Reflection data for Ferreal's Rust bindings",
		Options = UhtExporterOptions.Default,
		// An exporter in a UBT plugin must name a module, and runs only when that module is in
		// the build; every build with UObjects has CoreUObject. The output is the project's,
		// not the module's (see Output above).
		ModuleName = "CoreUObject")]
	public static void Export(IUhtExportFactory factory)
	{
		string project = factory.Session.ProjectDirectory;
		if (string.IsNullOrEmpty(project))
		{
			return;
		}
		// The other plugins' copies run in the same session (in turn): one export is enough.
		const string ExportedKey = "Ferreal.ExportedSession";
		if (ReferenceEquals(AppDomain.CurrentDomain.GetData(ExportedKey), factory.Session))
		{
			return;
		}
		AppDomain.CurrentDomain.SetData(ExportedKey, factory.Session);

		// Written here rather than through the factory, whose outputs are moved into place once
		// UHT is done: the cargo builds below read it.
		string path = Path.Combine(project, "Intermediate", "Ferreal", "FerrealEngine.json");
		WriteIfChanged(path, Json(factory.Session));
		BuildPending(Path.Combine(project, "Intermediate", "Ferreal", "Pending"), path);
	}

	/// <summary>The reflection data of everything in the session, as ferreal-build reads it.</summary>
	public static string Json(UhtSession session)
	{
		using var stream = new MemoryStream();
		var options = new JsonWriterOptions
		{
			Indented = false,
			Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping,
		};
		using (var json = new Utf8JsonWriter(stream, options))
		{
			json.WriteStartObject();
			json.WriteString("format", "ferreal-engine");
			json.WriteNumber("version", FormatVersion);

			var enums = new List<(UhtEnum Enum, string Module, UhtHeaderFile Header)>();
			var structs = new List<(UhtScriptStruct Struct, string Module, UhtHeaderFile Header)>();
			json.WriteStartArray("classes");
			foreach (UhtModule module in session.Modules)
			{
				foreach (UhtHeaderFile header in module.Headers)
				{
					foreach (UhtType type in header.Children)
					{
						switch (type)
						{
							case UhtClass c when c.ClassType == UhtClassType.Class:
								WriteClass(json, c, module.ShortName, header);
								break;
							case UhtEnum e:
								enums.Add((e, module.ShortName, header));
								break;
							case UhtScriptStruct s:
								structs.Add((s, module.ShortName, header));
								break;
						}
					}
				}
			}
			json.WriteEndArray();

			json.WriteStartArray("structs");
			foreach ((UhtScriptStruct s, string module, UhtHeaderFile header) in structs)
			{
				WriteStruct(json, s, module, header);
			}
			json.WriteEndArray();

			json.WriteStartArray("enums");
			foreach ((UhtEnum e, string module, UhtHeaderFile header) in enums)
			{
				WriteEnum(json, e, module, header);
			}
			json.WriteEndArray();

			// The modules, with the plugins they are in: what a module whose crate uses their
			// types depends on (ferreal-build lists them for its Build.cs and .uplugin).
			json.WriteStartArray("modules");
			foreach (UhtModule module in session.Modules)
			{
				json.WriteStartObject();
				json.WriteString("name", module.ShortName);
				string plugin = PluginOf(module.Module.BaseDirectory);
				if (plugin != null)
				{
					json.WriteString("plugin", plugin);
				}
				json.WriteEndObject();
			}
			json.WriteEndArray();

			json.WriteEndObject();
		}

		return Encoding.UTF8.GetString(stream.ToArray());
	}

	/// <summary>
	/// The plugin a module (in Directory) is in: the .uplugin beside the Source folder above it.
	/// Null for the engine's own modules and the project's.
	/// </summary>
	private static string PluginOf(string directory)
	{
		for (DirectoryInfo dir = string.IsNullOrEmpty(directory) ? null : new DirectoryInfo(directory); dir != null; dir = dir.Parent)
		{
			if (dir.Name.Equals("Source", StringComparison.OrdinalIgnoreCase))
			{
				FileInfo plugin = dir.Parent?.GetFiles("*.uplugin").FirstOrDefault();
				return plugin == null ? null : Path.GetFileNameWithoutExtension(plugin.Name);
			}
		}
		return null;
	}

	private static void WriteIfChanged(string path, string contents)
	{
		if (File.Exists(path) && File.ReadAllText(path) == contents)
		{
			return;
		}
		Directory.CreateDirectory(Path.GetDirectoryName(path));
		File.WriteAllText(path, contents);
	}

	/// <summary>
	/// Builds the crates a module's Build.cs left in Pending (a file per module: the crate's
	/// directory, cargo's arguments, and the source file with which the Build.cs failed the
	/// build), with the reflection data at Metadata. A crate that builds lifts that failure (the
	/// file is emptied: it is compiled in this build); one that fails fails the build, with
	/// cargo's errors.
	/// </summary>
	private static void BuildPending(string pending, string metadata)
	{
		if (!Directory.Exists(pending))
		{
			return;
		}
		foreach (string file in Directory.GetFiles(pending, "*.txt").Order())
		{
			string[] lines = File.ReadAllLines(file);
			File.Delete(file);
			if (lines.Length < 3)
			{
				continue;
			}
			var start = new ProcessStartInfo("cargo", lines[1])
			{
				WorkingDirectory = lines[0],
				UseShellExecute = false,
				RedirectStandardOutput = true,
				RedirectStandardError = true,
				CreateNoWindow = true,
			};
			start.Environment["FERREAL_ENGINE_METADATA"] = metadata;
			// Unreal Header Tool has read the headers, Unreal Build Tool listed the sources:
			// ferreal-build removes nothing, and fails on what comes too late (FERREAL_DEFERRED).
			start.Environment["FERREAL_DEFERRED"] = "1";
			Console.WriteLine("Rust: cargo " + lines[1]);
			using Process cargo = Process.Start(start);
			// Both streams at once: cargo can fill one while the other is waited on.
			Task<string> output = cargo.StandardOutput.ReadToEndAsync();
			string errors = cargo.StandardError.ReadToEnd();
			cargo.WaitForExit();
			if (cargo.ExitCode != 0)
			{
				throw new UhtException("Rust: cargo failed, with the reflection data just exported (" + Path.GetFileNameWithoutExtension(file) + "):\n" + output.Result + errors);
			}
			File.WriteAllText(lines[2], "// Written by Ferreal's exporter: the crate built with the reflection data it exported.\n");
		}
	}

	private static void WriteClass(Utf8JsonWriter json, UhtClass c, string module, UhtHeaderFile header)
	{
		json.WriteStartObject();
		json.WriteString("cpp", c.SourceName);
		json.WriteString("path", $"/Script/{module}.{c.EngineName}");
		json.WriteString("module", module);
		WriteHeader(json, header);
		if (c.SuperClass != null)
		{
			json.WriteString("super", c.SuperClass.SourceName);
		}
		json.WriteBoolean("abstract", c.ClassFlags.HasAnyFlags(EClassFlags.Abstract));
		json.WriteBoolean("deprecated", c.ClassFlags.HasAnyFlags(EClassFlags.Deprecated));
		WriteDoc(json, c);

		json.WriteStartArray("functions");
		foreach (UhtFunction f in c.Functions)
		{
			if (Exported(f))
			{
				WriteFunction(json, f);
			}
		}
		json.WriteEndArray();
		json.WriteEndObject();
	}

	private static void WriteHeader(Utf8JsonWriter json, UhtHeaderFile header)
	{
		string include = header.IncludeFilePath ?? header.ModuleRelativeFilePath;
		if (!string.IsNullOrEmpty(include))
		{
			json.WriteString("header", include);
		}
	}

	/// <summary>
	/// A struct and its reflected fields. ferreal-build copies the fields Rust can carry; it
	/// needs to know which C++ can reach (public, not editor-only, not deprecated, not arrays
	/// of a fixed size), and which structs are declared to UHT apart from their real C++
	/// (NoExport: the core math types).
	/// </summary>
	private static void WriteStruct(Utf8JsonWriter json, UhtScriptStruct s, string module, UhtHeaderFile header)
	{
		json.WriteStartObject();
		json.WriteString("cpp", s.SourceName);
		json.WriteString("path", $"/Script/{module}.{s.EngineName}");
		json.WriteString("module", module);
		WriteHeader(json, header);
		if (s.SuperScriptStruct != null)
		{
			json.WriteString("super", s.SuperScriptStruct.SourceName);
		}
		json.WriteBoolean("noexport", s.ScriptStructFlags.HasAnyFlags(EStructFlags.NoExport));
		WriteDoc(json, s);
		json.WriteStartArray("fields");
		foreach (UhtProperty p in s.Properties)
		{
			EPropertyFlags flags = p.PropertyFlags;
			json.WriteStartObject();
			json.WriteString("name", p.SourceName);
			json.WritePropertyName("type");
			WriteType(json, p);
			json.WriteBoolean("public", flags.HasAnyFlags(EPropertyFlags.NativeAccessSpecifierPublic));
			json.WriteBoolean("editor_only", flags.HasAnyFlags(EPropertyFlags.EditorOnly));
			json.WriteBoolean("deprecated", flags.HasAnyFlags(EPropertyFlags.Deprecated));
			json.WriteBoolean("fixed_array", p.ArrayDimensions != null);
			WriteDoc(json, p);
			json.WriteEndObject();
		}
		json.WriteEndArray();
		json.WriteEndObject();
	}

	/// <summary>Functions reflection can call: not delegates or network messages, not private.</summary>
	private static bool Exported(UhtFunction f)
	{
		if (f.FunctionType != UhtFunctionType.Function)
		{
			return false;
		}
		if (f.FunctionFlags.HasAnyFlags(EFunctionFlags.Delegate | EFunctionFlags.Private | EFunctionFlags.NetRequest | EFunctionFlags.NetResponse))
		{
			return false;
		}
		return f.FunctionFlags.HasAnyFlags(EFunctionFlags.Native | EFunctionFlags.BlueprintEvent);
	}

	private static void WriteFunction(Utf8JsonWriter json, UhtFunction f)
	{
		EFunctionFlags flags = f.FunctionFlags;
		json.WriteStartObject();
		json.WriteString("cpp", f.SourceName);
		json.WriteString("name", f.EngineName);
		json.WriteBoolean("static", flags.HasAnyFlags(EFunctionFlags.Static));
		json.WriteBoolean("const", flags.HasAnyFlags(EFunctionFlags.Const));
		json.WriteBoolean("callable", flags.HasAnyFlags(EFunctionFlags.BlueprintCallable));
		json.WriteBoolean("pure", flags.HasAnyFlags(EFunctionFlags.BlueprintPure));
		json.WriteBoolean("event", flags.HasAnyFlags(EFunctionFlags.BlueprintEvent));
		json.WriteBoolean("protected", flags.HasAnyFlags(EFunctionFlags.Protected));
		json.WriteBoolean("net", flags.HasAnyFlags(EFunctionFlags.Net));
		json.WriteBoolean("editor_only", flags.HasAnyFlags(EFunctionFlags.EditorOnly));
		json.WriteBoolean("internal", f.MetaData.ContainsKey("BlueprintInternalUseOnly"));
		if (f.MetaData.TryGetValue("DeprecationMessage", out string message))
		{
			json.WriteString("deprecated", message);
		}
		else if (f.MetaData.ContainsKey("DeprecatedFunction"))
		{
			json.WriteString("deprecated", "");
		}
		WriteDoc(json, f);

		json.WriteStartArray("params");
		foreach (UhtType child in f.ParameterProperties.Span)
		{
			if (child is UhtProperty p)
			{
				WriteParam(json, p, f, false);
			}
		}
		if (f.ReturnProperty != null)
		{
			WriteParam(json, f.ReturnProperty, f, true);
		}
		json.WriteEndArray();
		json.WriteEndObject();
	}

	private static void WriteParam(Utf8JsonWriter json, UhtProperty p, UhtFunction f, bool isReturn)
	{
		EPropertyFlags flags = p.PropertyFlags;
		json.WriteStartObject();
		json.WriteString("name", p.SourceName);
		json.WritePropertyName("type");
		WriteType(json, p);
		json.WriteBoolean("ret", isReturn);
		json.WriteBoolean("out", !isReturn && flags.HasAnyFlags(EPropertyFlags.OutParm));
		json.WriteBoolean("const", flags.HasAnyFlags(EPropertyFlags.ConstParm));
		json.WriteBoolean("ref", flags.HasAnyFlags(EPropertyFlags.ReferenceParm));
		if (!isReturn && f.MetaData.TryGetValue($"CPP_Default_{p.SourceName}", out string value))
		{
			json.WriteString("default", value);
		}
		json.WriteEndObject();
	}

	/// <summary>
	/// A property's type: its kind, and the engine class of its FProperty (what the generated
	/// code checks at run time), with the class, struct, enum or element it refers to.
	/// </summary>
	private static void WriteType(Utf8JsonWriter json, UhtProperty p)
	{
		json.WriteStartObject();
		switch (p)
		{
			case UhtBoolProperty:
				Kind(json, "bool", "BoolProperty");
				break;
			case UhtByteProperty b:
				Kind(json, b.Enum != null ? "enum" : "uint8", "ByteProperty");
				if (b.Enum != null)
				{
					json.WriteString("enum", b.Enum.SourceName);
				}
				break;
			case UhtInt8Property:
				Kind(json, "int8", "Int8Property");
				break;
			case UhtInt16Property:
				Kind(json, "int16", "Int16Property");
				break;
			case UhtIntProperty:
				Kind(json, "int32", "IntProperty");
				break;
			case UhtInt64Property:
				Kind(json, "int64", "Int64Property");
				break;
			case UhtUInt16Property:
				Kind(json, "uint16", "UInt16Property");
				break;
			case UhtUInt32Property:
				Kind(json, "uint32", "UInt32Property");
				break;
			case UhtUInt64Property:
				Kind(json, "uint64", "UInt64Property");
				break;
			case UhtFloatProperty:
				Kind(json, "float", "FloatProperty");
				break;
			case UhtLargeWorldCoordinatesRealProperty:
				Kind(json, "double", "LargeWorldCoordinatesRealProperty");
				break;
			case UhtDoubleProperty:
				Kind(json, "double", "DoubleProperty");
				break;
			case UhtStrProperty:
				Kind(json, "string", "StrProperty");
				break;
			case UhtNameProperty:
				Kind(json, "name", "NameProperty");
				break;
			case UhtTextProperty:
				Kind(json, "text", "TextProperty");
				break;
			case UhtEnumProperty e:
				// An EnumProperty for an enum class; a ByteProperty (as TEnumAsByte) for an old
				// namespaced or plain enum, whatever its declared type.
				Kind(json, "enum", e.EngineClassName);
				json.WriteString("enum", e.Enum.SourceName);
				break;
			case UhtStructProperty s:
				Kind(json, "struct", "StructProperty");
				json.WriteString("struct", s.ScriptStruct.SourceName);
				break;
			case UhtClassProperty c:
				Kind(json, "class", "ClassProperty");
				json.WriteString("class", c.MetaClass?.SourceName ?? "UObject");
				break;
			case UhtSoftClassProperty:
				Kind(json, "other", "SoftClassProperty");
				break;
			case UhtSoftObjectProperty:
				Kind(json, "other", "SoftObjectProperty");
				break;
			case UhtWeakObjectPtrProperty w:
				Kind(json, "weak", "WeakObjectProperty");
				json.WriteString("class", w.Class.SourceName);
				break;
			case UhtLazyObjectPtrProperty:
				Kind(json, "other", "LazyObjectProperty");
				break;
			case UhtInterfaceProperty:
				Kind(json, "other", "InterfaceProperty");
				break;
			case UhtObjectProperty o:
				// A raw pointer or a TObjectPtr: a pointer either way.
				Kind(json, "object", "ObjectProperty");
				json.WriteString("class", o.Class.SourceName);
				break;
			case UhtArrayProperty a:
				Kind(json, "array", "ArrayProperty");
				json.WritePropertyName("inner");
				WriteType(json, a.ValueProperty);
				break;
			default:
				Kind(json, "other", p.EngineClassName);
				break;
		}
		json.WriteEndObject();
	}

	private static void Kind(Utf8JsonWriter json, string kind, string property)
	{
		json.WriteString("kind", kind);
		json.WriteString("property", property);
	}

	/// <summary>
	/// An enum: its form, which decides how C++ names it (`ECollisionEnabled::Type` for an old
	/// namespaced one) and how reflection holds it (an enum class as itself, any other as a
	/// byte), and its values.
	/// </summary>
	private static void WriteEnum(Utf8JsonWriter json, UhtEnum e, string module, UhtHeaderFile header)
	{
		json.WriteStartObject();
		json.WriteString("cpp", e.SourceName);
		json.WriteString("path", $"/Script/{module}.{e.EngineName}");
		WriteHeader(json, header);
		json.WriteString("form", e.CppForm switch
		{
			UhtEnumCppForm.EnumClass => "enum_class",
			UhtEnumCppForm.Namespaced => "namespaced",
			_ => "regular",
		});
		json.WriteString("type", e.FullyQualifiedCppType);
		json.WriteString("underlying", e.UnderlyingType switch
		{
			UhtEnumUnderlyingType.Uint8 => "uint8",
			UhtEnumUnderlyingType.Int8 => "int8",
			UhtEnumUnderlyingType.Int16 => "int16",
			UhtEnumUnderlyingType.Uint16 => "uint16",
			UhtEnumUnderlyingType.Int32 => "int32",
			UhtEnumUnderlyingType.Uint32 => "uint32",
			UhtEnumUnderlyingType.Int64 => "int64",
			UhtEnumUnderlyingType.Uint64 => "uint64",
			// `: int`: 32 bits on every platform Unreal supports.
			UhtEnumUnderlyingType.Int => "int32",
			_ => "unspecified",
		});
		WriteDoc(json, e);
		json.WriteStartArray("values");
		foreach (UhtEnumValue v in e.EnumValues)
		{
			// "EFoo::Bar" and "Bar" alike: the name after the last "::". UHT records -1 for a
			// value it could not evaluate (an expression); ferreal-build leaves those out.
			string name = v.Name;
			int colon = name.LastIndexOf("::", StringComparison.Ordinal);
			if (colon >= 0)
			{
				name = name[(colon + 2)..];
			}
			json.WriteStartObject();
			json.WriteString("name", name);
			json.WriteNumber("value", v.Value);
			json.WriteEndObject();
		}
		json.WriteEndArray();
		json.WriteEndObject();
	}

	private static void WriteDoc(Utf8JsonWriter json, UhtType type)
	{
		if (type.MetaData.TryGetValue("ToolTip", out string tip) && !string.IsNullOrWhiteSpace(tip))
		{
			json.WriteString("doc", tip);
		}
	}
}
