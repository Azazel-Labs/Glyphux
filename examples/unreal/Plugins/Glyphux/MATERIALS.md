# Glyphux materials

Glyphux draws text in materials: one Custom node calls a function of
`Shaders/Private/GlyphuxMaterial.ush`, which reads the atlas and the text's records from
textures the plugin fills, and returns the pixel's color (linear) and opacity. There are no
render targets between the text and the screen: every widget, text component, particle and
surface samples the same atlas pages, so the text stays sharp at any distance and on any
shape.

The plugin needs six materials and two material functions. `Content/Python/glyphux_materials.py`
makes them all; run it once in the editor (with the Python Editor Script Plugin enabled):
Tools > Execute Python Script, then the file. This page says what it builds, so you can build
or change them by hand.

| Asset | For | Function called |
| --- | --- | --- |
| `M_GlyphuxUI` | Glyphux Text widgets (UMG) | `GlyphuxShade` |
| `M_GlyphuxWorld` | Glyphux Text components | `GlyphuxShadeGlyph` |
| `M_GlyphuxWorldNoDepthTest` | components with No Depth Test | `GlyphuxShadeGlyph` |
| `M_GlyphuxGlyph` | one glyph per Niagara sprite or mesh fragment | `GlyphuxShadeGlyph` |
| `M_GlyphuxSurface` | a whole text on any mesh | `GlyphuxShadeSurface` |
| `MF_GlyphuxGlyph`, `MF_GlyphuxSurface` | the last two inside your own materials | same |
| `T_GlyphuxLinearDefault` | the texture parameters' default (1x1, linear) | |

Project Settings > Plugins > Glyphux names the materials the plugin uses; the defaults are
the paths above, in the plugin's content (`/Glyphux/Materials`).

## The parameters

The plugin sets these on the material instances it makes (and on yours, with Bind To
Material). A material needs only those its Custom node takes.

| Parameter | Type | Default | What |
| --- | --- | --- | --- |
| `GlyphuxAtlas` | Texture Object | `T_GlyphuxLinearDefault` | the atlas pages, stacked |
| `GlyphuxColorPages` | Texture Object | same | the color glyph pages |
| `GlyphuxRecords` | Texture Object | same | the text's glyph records |
| `GlyphuxTables` | Texture Object | same | its color glyph tables |
| `GlyphuxDrawQuads` | Texture Object | same | all its quads (world) |
| `GlyphuxSurfaceQuads` | Texture Object | same | its glyphs' quads (glyph and surface) |
| `GlyphuxSurfaceGrid` | Texture Object | same | the grid over them (surface) |
| `GlyphuxPxRange` | Scalar | 4 | the fields' range |
| `GlyphuxPages` | Scalar | 6 | atlas pages |
| `GlyphuxTextBox` | Vector | (0, 0, 512, 128) | the text's box: left, top, width, height (surface) |

Every texture parameter's Sampler Type is **Linear Color**: the textures are linear (the
float ones are read with Load, never filtered). Make `T_GlyphuxLinearDefault` any 1x1 texture
with sRGB off, so the defaults match that sampler type.

## The Custom node

Every material is the same few nodes:

1. A **Custom** node: Output Type `CMOT Float 4`, Include File Paths
   `/Plugin/Glyphux/Private/GlyphuxMaterial.ush`, and the inputs and code below (the input
   names must be exactly these).
2. Its result's RGB (a Component Mask) times the **Vertex Color** RGB into **Emissive Color**,
   its A times the Vertex Color's A into **Opacity**. The vertex color is the widget's tint,
   or the particle's color.

Texture Object inputs give the code the texture and its sampler (`Atlas` and
`AtlasSampler`), which the functions take as arguments.

**UI** (`M_GlyphuxUI`): inputs `Atlas`, `Records`, `ColorPages`, `Tables` (the texture
parameters above), `UV` (Texture Coordinate 0), `Info` (Texture Coordinate 1), `PxRange`,
`Pages`, `Time` (a Time node):

```hlsl
return GlyphuxShade(Atlas, AtlasSampler, Records, ColorPages, ColorPagesSampler, Tables, UV, Info, PxRange, Pages, Time);
```

**World** (`M_GlyphuxWorld`, `M_GlyphuxWorldNoDepthTest`): a procedural mesh keeps texture
coordinates at half precision, too coarse for the atlas, so the component's vertices carry
only exact numbers: their corner of the quad (Texture Coordinate 0) and the quad's index
(Texture Coordinate 1: the index mod 2048, then the index over 2048). The quad's texture
coordinates, record and pass come from `GlyphuxDrawQuads`. Inputs `Atlas`, `Records`,
`ColorPages`, `Tables`, `DrawQuads`, `UV` (Texture Coordinate 0), `Info` (Texture
Coordinate 1), `PxRange`, `Pages`, `Time`:

```hlsl
return GlyphuxShadeGlyph(Atlas, AtlasSampler, Records, ColorPages, ColorPagesSampler, Tables, DrawQuads, UV, Info.x + Info.y * 2048.0, PxRange, Pages, Time);
```

**Glyph** (`M_GlyphuxGlyph`): inputs `Atlas`, `Records`, `ColorPages`, `Tables`,
`SurfaceQuads`, `UV` (Texture Coordinate 0: the sprite's, or the fragment mesh's), `Quad`
(a **Dynamic Parameter** node, its first parameter renamed `GlyphuxQuad`, default -1),
`PxRange`, `Pages`, `Time`:

```hlsl
return GlyphuxShadeGlyph(Atlas, AtlasSampler, Records, ColorPages, ColorPagesSampler, Tables, SurfaceQuads, UV, Quad, PxRange, Pages, Time);
```

**Surface** (`M_GlyphuxSurface`): inputs `Atlas`, `Records`, `ColorPages`, `Tables`,
`SurfaceQuads`, `SurfaceGrid`, `UV` (Texture Coordinate 0, or any channel), `Box` (the
`GlyphuxTextBox` vector parameter, all four channels: Append its R and G, its B and A, then
the two), `PxRange`, `Pages`, `Time`:

```hlsl
return GlyphuxShadeSurface(Atlas, AtlasSampler, Records, ColorPages, ColorPagesSampler, Tables, SurfaceQuads, SurfaceGrid, Box.xy + UV * Box.zw, PxRange, Pages, Time);
```

## The material settings

* `M_GlyphuxUI`: Material Domain **User Interface**, Blend Mode **Translucent**.
* The others: Material Domain **Surface**, Blend Mode **Translucent**, Shading Model
  **Unlit**, **Two Sided**. `M_GlyphuxWorldNoDepthTest` also has **Disable Depth Test**.
* `M_GlyphuxGlyph`: Usage **Used with Niagara Sprites** and **Used with Niagara Mesh
  Particles**.
* `M_GlyphuxSurface`: Usage **Used with Geometry Collections** (so a fractured mesh's pieces
  keep the text), **Used with Skeletal Mesh** and **Used with Niagara Mesh Particles**.

## Text on any surface

A Glyphux Text component lays the text out; a mesh with the surface material shows it, by its
texture coordinates: (0, 0) is the text box's top-left corner, (1, 1) its bottom-right. The
mesh can be anything, curved, skinned, or a Chaos Geometry Collection: the pieces keep their
texture coordinates, so when it breaks, the text breaks with it, each piece drawing its own
part of the glyphs at full sharpness.

1. Add a Glyphux Text component (to the mesh's actor, say) and set its text. Turn **Draw
   Text** off: it lays out without drawing itself. Set **Fixed Scale** to how many screen
   pixels a layout pixel will cover on the surface (1 for the font size in pixels on screen,
   2 for twice it): the component cannot measure it on someone else's mesh.
2. Call **Get Surface Material** on the component and set the result on the mesh's material
   slot, in the construction script or Begin Play. The instance follows the text: change the
   text and the surface changes.

The box's aspect is the text's; stretch or tile the mesh's texture coordinates to suit (a
cylinder's go once around). For another channel, or a scale and offset, use the material
function in a material of your own.

Each pixel looks the text point up in a grid over the glyphs (a few glyphs a cell), finds the
glyph quads that hold it (inverting each quad's corners), and draws them in order, so glows
and shadows that reach a neighbor overlap as they do on the component. At most 12 quads
overlap at a point (`GLYPHUX_SURFACE_MAX_QUADS` in `GlyphuxMaterial.ush`). Wave and shake animations do not move on surfaces;
rainbow does.

## Text in your own materials

`MF_GlyphuxSurface` and `MF_GlyphuxGlyph` return **Color** (linear) and **Opacity** (the
text's coverage, with its outline, glow and shadow). Put text onto a lit, opaque prop:

* Base Color: Lerp from your base color to Color by Opacity.
* Emissive: Color times Opacity times a brightness, for text that glows in the dark.
* Roughness, Metallic: Lerp to the paint's, by Opacity.

`MF_GlyphuxSurface` takes **UV** (default: Texture Coordinate 0), 0 to 1 across the text box;
`MF_GlyphuxGlyph` takes **UV** and **Quad** (default: the particle's first dynamic
parameter). Make a Material Instance Dynamic of your material and call **Bind To Material**
on the text component with it: the component sets the parameters now and whenever the text
changes.

## Where the functions are

`Shaders/Private/GlyphuxMaterial.ush` holds the material functions: `GlyphuxShade`,
`GlyphuxShadeGlyph`, `GlyphuxShadeSurface`, and what they share. The shading itself is
glyphux's, translated from its WGSL by naga when the plugin's Rust is built
(`Shaders/Private/Generated/GlyphuxTextCore.ush` and `GlyphuxColorCore.ush`): the same code
draws the text in the web demo, in Godot and here.
