"""Creates the Glyphux plugin's materials, as MATERIALS.md describes them.

Run it once in the editor, with the Python Editor Script Plugin enabled:
Tools > Execute Python Script, then this file (or `py glyphux_materials.py` in the console;
the plugin's Content/Python folder is on Unreal's Python path). Running it again rebuilds the
materials.

It writes, into the plugin's content (/Glyphux/Materials):

* T_GlyphuxLinearDefault: a 1x1 linear texture, the default of the texture parameters (their
  sampler type is Linear Color, which an sRGB default texture would not match);
* M_GlyphuxUI: the material of Glyphux Text widgets (User Interface domain);
* M_GlyphuxWorld and M_GlyphuxWorldNoDepthTest: the materials of Glyphux Text components;
* M_GlyphuxGlyph: one glyph per Niagara sprite or mesh fragment;
* M_GlyphuxSurface: a whole text on any mesh, by its texture coordinates;
* MF_GlyphuxGlyph and MF_GlyphuxSurface: the same two as material functions (Color and
  Opacity out), to put text into materials of your own (lit, opaque, over a base color).

Every one is a Custom node calling a function of Shaders/Private/GlyphuxMaterial.ush with the
plugin's textures; the materials take its color and opacity, times the vertex color (the
widget's tint, the particle's color).
"""

import os
import struct
import tempfile
import zlib

import unreal

FOLDER = "/Glyphux/Materials"
INCLUDE = "/Plugin/Glyphux/Private/GlyphuxMaterial.ush"
TEXTURES = "Atlas, AtlasSampler, Records, ColorPages, ColorPagesSampler, Tables"

# What each kind of material computes: its Custom node's code and inputs.
TEXT = {
    "code": "return GlyphuxShade(" + TEXTURES + ", UV, Info, PxRange, Pages, Time);",
    "inputs": ["Atlas", "Records", "ColorPages", "Tables", "UV", "Info", "PxRange", "Pages", "Time"],
}
# The world materials: a procedural mesh keeps texture coordinates at half precision, so its
# vertices carry only their corner (texture coordinate 0) and the quad's index (texture
# coordinate 1: index mod 2048, index / 2048), and the rest comes from GlyphuxDrawQuads.
WORLD = {
    "code": "return GlyphuxShadeGlyph(" + TEXTURES + ", DrawQuads, UV, Info.x + Info.y * 2048.0, PxRange, Pages, Time);",
    "inputs": ["Atlas", "Records", "ColorPages", "Tables", "DrawQuads", "UV", "Info", "PxRange", "Pages", "Time"],
}
GLYPH = {
    "code": "return GlyphuxShadeGlyph(" + TEXTURES + ", SurfaceQuads, UV, Quad, PxRange, Pages, Time);",
    "inputs": ["Atlas", "Records", "ColorPages", "Tables", "SurfaceQuads", "UV", "Quad", "PxRange", "Pages", "Time"],
}
SURFACE = {
    "code": "return GlyphuxShadeSurface(" + TEXTURES + ", SurfaceQuads, SurfaceGrid, Box.xy + UV * Box.zw, PxRange, Pages, Time);",
    "inputs": ["Atlas", "Records", "ColorPages", "Tables", "SurfaceQuads", "SurfaceGrid", "UV", "Box", "PxRange", "Pages", "Time"],
}

MEL = unreal.MaterialEditingLibrary
ASSETS = unreal.EditorAssetLibrary


def png_1x1(rgba):
    """A 1x1 RGBA PNG."""

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    header = struct.pack(">IIBBBBB", 1, 1, 8, 6, 0, 0, 0)
    pixels = zlib.compress(b"\x00" + bytes(rgba))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", pixels) + chunk(b"IEND", b"")


def linear_default_texture():
    path = FOLDER + "/T_GlyphuxLinearDefault"
    if ASSETS.does_asset_exist(path):
        return ASSETS.load_asset(path)
    folder = tempfile.mkdtemp()
    file = os.path.join(folder, "T_GlyphuxLinearDefault.png")
    with open(file, "wb") as f:
        f.write(png_1x1([0, 0, 0, 0]))
    task = unreal.AssetImportTask()
    task.set_editor_property("filename", file)
    task.set_editor_property("destination_path", FOLDER)
    task.set_editor_property("automated", True)
    task.set_editor_property("save", False)
    task.set_editor_property("replace_existing", True)
    unreal.AssetToolsHelpers.get_asset_tools().import_asset_tasks([task])
    texture = ASSETS.load_asset(path)
    texture.set_editor_property("srgb", False)
    texture.set_editor_property("compression_settings", unreal.TextureCompressionSettings.TC_VECTOR_DISPLACEMENTMAP)
    texture.set_editor_property("filter", unreal.TextureFilter.TF_NEAREST)
    ASSETS.save_loaded_asset(texture)
    return texture


def material(name):
    """The material at FOLDER/name, emptied, or a new one."""
    path = FOLDER + "/" + name
    if ASSETS.does_asset_exist(path):
        m = ASSETS.load_asset(path)
        MEL.delete_all_material_expressions(m)
        return m
    tools = unreal.AssetToolsHelpers.get_asset_tools()
    return tools.create_asset(name, FOLDER, unreal.Material, unreal.MaterialFactoryNew())


def material_function(name):
    """The material function at FOLDER/name, emptied, or a new one."""
    path = FOLDER + "/" + name
    if ASSETS.does_asset_exist(path):
        f = ASSETS.load_asset(path)
        MEL.delete_all_material_expressions_in_function(f)
        return f
    tools = unreal.AssetToolsHelpers.get_asset_tools()
    return tools.create_asset(name, FOLDER, unreal.MaterialFunction, unreal.MaterialFunctionFactoryNew())


class Graph:
    """Creates the expressions of a material or of a material function."""

    def __init__(self, owner):
        self.owner = owner
        self.function = isinstance(owner, unreal.MaterialFunction)

    def add(self, cls, x, y):
        if self.function:
            return MEL.create_material_expression_in_function(self.owner, cls, x, y)
        return MEL.create_material_expression(self.owner, cls, x, y)

    def texture_parameter(self, name, default, y):
        p = self.add(unreal.MaterialExpressionTextureObjectParameter, -900, y)
        p.set_editor_property("parameter_name", name)
        p.set_editor_property("texture", default)
        p.set_editor_property("sampler_type", unreal.MaterialSamplerType.SAMPLERTYPE_LINEAR_COLOR)
        return p

    def scalar_parameter(self, name, value, y):
        p = self.add(unreal.MaterialExpressionScalarParameter, -900, y)
        p.set_editor_property("parameter_name", name)
        p.set_editor_property("default_value", value)
        return p

    def texture_coordinate(self, index, y):
        c = self.add(unreal.MaterialExpressionTextureCoordinate, -1100, y)
        c.set_editor_property("coordinate_index", index)
        return c

    def function_input(self, name, kind, preview, priority, y):
        """A function input that defaults to preview when nothing is connected to it."""
        i = self.add(unreal.MaterialExpressionFunctionInput, -900, y)
        i.set_editor_property("input_name", name)
        i.set_editor_property("input_type", kind)
        i.set_editor_property("sort_priority", priority)
        i.set_editor_property("use_preview_value_as_default", True)
        MEL.connect_material_expressions(preview, "", i, "Preview")
        return i


def text_box(g, y):
    """GlyphuxTextBox (left, top, width, height, layout units) as a float4."""
    p = g.add(unreal.MaterialExpressionVectorParameter, -1300, y)
    p.set_editor_property("parameter_name", "GlyphuxTextBox")
    p.set_editor_property("default_value", unreal.LinearColor(0.0, 0.0, 512.0, 128.0))
    rg = g.add(unreal.MaterialExpressionAppendVector, -1100, y)
    MEL.connect_material_expressions(p, "R", rg, "A")
    MEL.connect_material_expressions(p, "G", rg, "B")
    ba = g.add(unreal.MaterialExpressionAppendVector, -1100, y + 80)
    MEL.connect_material_expressions(p, "B", ba, "A")
    MEL.connect_material_expressions(p, "A", ba, "B")
    box = g.add(unreal.MaterialExpressionAppendVector, -900, y)
    MEL.connect_material_expressions(rg, "", box, "A")
    MEL.connect_material_expressions(ba, "", box, "B")
    return box


def quad_parameter(g, y):
    """The particle's quad index: its first dynamic material parameter (-1 draws nothing)."""
    d = g.add(unreal.MaterialExpressionDynamicParameter, -1100, y)
    d.set_editor_property("param_names", ["GlyphuxQuad", "Param2", "Param3", "Param4"])
    d.set_editor_property("default_value", unreal.LinearColor(-1.0, 0.0, 0.0, 0.0))
    return d


def build(g, kind, default):
    """The Custom node of kind (TEXT, GLYPH or SURFACE) and what feeds it."""
    custom = g.add(unreal.MaterialExpressionCustom, -450, 0)
    custom.set_editor_property("code", kind["code"])
    custom.set_editor_property("output_type", unreal.CustomMaterialOutputType.CMOT_FLOAT4)
    custom.set_editor_property("description", kind["code"].split("(")[0].replace("return ", ""))
    custom.set_editor_property("include_file_paths", [INCLUDE])
    inputs = []
    for n in kind["inputs"]:
        i = unreal.CustomInput()
        i.set_editor_property("input_name", n)
        inputs.append(i)
    custom.set_editor_property("inputs", inputs)

    sources = {
        "Atlas": lambda: g.texture_parameter("GlyphuxAtlas", default, -500),
        "Records": lambda: g.texture_parameter("GlyphuxRecords", default, -350),
        "ColorPages": lambda: g.texture_parameter("GlyphuxColorPages", default, -200),
        "Tables": lambda: g.texture_parameter("GlyphuxTables", default, -50),
        "DrawQuads": lambda: g.texture_parameter("GlyphuxDrawQuads", default, 100),
        "SurfaceQuads": lambda: g.texture_parameter("GlyphuxSurfaceQuads", default, 100),
        "SurfaceGrid": lambda: g.texture_parameter("GlyphuxSurfaceGrid", default, 250),
        "Info": lambda: g.texture_coordinate(1, 475),
        "Box": lambda: text_box(g, 550),
        "PxRange": lambda: g.scalar_parameter("GlyphuxPxRange", 4.0, 700),
        "Pages": lambda: g.scalar_parameter("GlyphuxPages", 6.0, 800),
        "Time": lambda: g.add(unreal.MaterialExpressionTime, -900, 900),
    }
    uv = g.texture_coordinate(0, 400)
    if g.function:
        uv = g.function_input("UV", unreal.FunctionInputType.FUNCTION_INPUT_VECTOR2, uv, 0, 400)
    sources["UV"] = lambda: (uv, "")
    if "Quad" in kind["inputs"]:
        quad = quad_parameter(g, 475)
        output = "GlyphuxQuad"
        if g.function:
            # The dynamic parameter, connected by name, is the input's default.
            quad = g.function_input("Quad", unreal.FunctionInputType.FUNCTION_INPUT_SCALAR, quad, 1, 475)
            output = ""
        sources["Quad"] = lambda: (quad, output)
    for n in kind["inputs"]:
        source = sources[n]()
        output = ""
        if isinstance(source, tuple):
            source, output = source
        if not MEL.connect_material_expressions(source, output, custom, n) and output:
            MEL.connect_material_expressions(source, "", custom, n)
    return custom


def split(g, custom):
    """The Custom node's color and opacity."""
    rgb = g.add(unreal.MaterialExpressionComponentMask, -200, -50)
    for c in ("r", "g", "b"):
        rgb.set_editor_property(c, True)
    alpha = g.add(unreal.MaterialExpressionComponentMask, -200, 100)
    alpha.set_editor_property("a", True)
    MEL.connect_material_expressions(custom, "", rgb, "")
    MEL.connect_material_expressions(custom, "", alpha, "")
    return rgb, alpha


def text_material(m, kind, default):
    """A material of kind: its color and opacity, times the vertex color."""
    g = Graph(m)
    rgb, alpha = split(g, build(g, kind, default))
    vertex = g.add(unreal.MaterialExpressionVertexColor, -200, 250)
    color = g.add(unreal.MaterialExpressionMultiply, 0, -50)
    MEL.connect_material_expressions(rgb, "", color, "A")
    MEL.connect_material_expressions(vertex, "", color, "B")
    opacity = g.add(unreal.MaterialExpressionMultiply, 0, 100)
    MEL.connect_material_expressions(alpha, "", opacity, "A")
    MEL.connect_material_expressions(vertex, "A", opacity, "B")
    MEL.connect_material_property(color, "", unreal.MaterialProperty.MP_EMISSIVE_COLOR)
    MEL.connect_material_property(opacity, "", unreal.MaterialProperty.MP_OPACITY)
    MEL.layout_material_expressions(m)
    MEL.recompile_material(m)
    ASSETS.save_loaded_asset(m)
    unreal.log("Glyphux: " + m.get_path_name())


def text_function(f, kind, default, description):
    """A material function of kind: Color (linear) and Opacity out."""
    g = Graph(f)
    rgb, alpha = split(g, build(g, kind, default))
    for name, source, y in (("Color", rgb, -50), ("Opacity", alpha, 100)):
        o = g.add(unreal.MaterialExpressionFunctionOutput, 100, y)
        o.set_editor_property("output_name", name)
        MEL.connect_material_expressions(source, "", o, "")
    f.set_editor_property("description", description)
    f.set_editor_property("expose_to_library", True)
    MEL.layout_material_function_expressions(f)
    MEL.update_material_function(f)
    ASSETS.save_loaded_asset(f)
    unreal.log("Glyphux: " + f.get_path_name())


def world(m, depth_test=True):
    """Surface domain, translucent, unlit, two sided."""
    m.set_editor_property("material_domain", unreal.MaterialDomain.MD_SURFACE)
    m.set_editor_property("blend_mode", unreal.BlendMode.BLEND_TRANSLUCENT)
    m.set_editor_property("shading_model", unreal.MaterialShadingModel.MSM_UNLIT)
    m.set_editor_property("two_sided", True)
    m.set_editor_property("disable_depth_test", not depth_test)


def main():
    default = linear_default_texture()

    ui = material("M_GlyphuxUI")
    ui.set_editor_property("material_domain", unreal.MaterialDomain.MD_UI)
    ui.set_editor_property("blend_mode", unreal.BlendMode.BLEND_TRANSLUCENT)
    text_material(ui, TEXT, default)

    for name, depth_test in (("M_GlyphuxWorld", True), ("M_GlyphuxWorldNoDepthTest", False)):
        w = material(name)
        world(w, depth_test)
        text_material(w, WORLD, default)

    glyph = material("M_GlyphuxGlyph")
    world(glyph)
    glyph.set_editor_property("used_with_niagara_sprites", True)
    glyph.set_editor_property("used_with_niagara_mesh_particles", True)
    text_material(glyph, GLYPH, default)

    surface = material("M_GlyphuxSurface")
    world(surface)
    for usage in ("used_with_geometry_collections", "used_with_skeletal_mesh", "used_with_niagara_mesh_particles"):
        surface.set_editor_property(usage, True)
    text_material(surface, SURFACE, default)

    text_function(
        material_function("MF_GlyphuxGlyph"),
        GLYPH,
        default,
        "One Glyphux glyph (a Niagara sprite or mesh fragment). UV: the texture coordinates in the glyph's square "
        "(default: texture coordinate 0). Quad: the glyph's quad index (default: the particle's first dynamic parameter).",
    )
    text_function(
        material_function("MF_GlyphuxSurface"),
        SURFACE,
        default,
        "A Glyphux text on any mesh. UV: 0 to 1 across the text's box (GlyphuxTextBox; default: texture coordinate 0). "
        "Color is linear, Opacity its coverage: lerp your base color to it.",
    )


main()
