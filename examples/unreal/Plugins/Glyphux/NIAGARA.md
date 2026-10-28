# Glyphux text in Niagara

A Niagara system can take a Glyphux text's glyphs and move them: a particle per glyph (a
sprite), or a particle per fragment of a glyph (a mesh), for text that scatters, bursts,
shatters or assembles. Every particle draws its glyph with the glyph material
(`M_GlyphuxGlyph`, see MATERIALS.md), which reads the atlas the text uses: no render targets,
and each fragment shows exactly its part of its glyph, sharp at any distance.

The text comes from a Glyphux Text component, which lays it out. **Set Niagara Glyphs**
(Glyphux Niagara Library) hands its glyphs to a Niagara component's user parameters; the
system spawns one particle per entry and places it from them.

## What Set Niagara Glyphs sets

Create these user parameters in the system (Parameters panel, User Exposed), with these
names and types:

| Name | Type | Per entry |
| --- | --- | --- |
| `GlyphuxCount` | int32 | how many entries (spawn that many) |
| `GlyphuxPositions` | Array Float3 | the particle's position at rest |
| `GlyphuxCenters` | Array Float3 | its glyph's center (fragments of one glyph share it) |
| `GlyphuxSizes` | Array Float2 | the glyph's quad's width and height |
| `GlyphuxQuads` | Array Float | the glyph's quad index, for the material |
| `GlyphuxMeshIndices` | Array Int32 | the fragment's mesh index (0 without fragments) |
| `GlyphuxRotation` | Quat | the text's rotation (glyphs face its +X, up is +Z) |
| `GlyphuxMaterial` | Material Interface | the glyph material, bound to the text |

Positions are world space (for an emitter with Local Space off); with **Local Space** on the
call, relative to the Niagara component. The call returns the entry count: 0 means the text
was not laid out yet (it lays out when its component first ticks), so call it from the text
component's **On Text Updated**, or a frame later.

The glyph quads include their glow and shadow: a particle draws the whole glyph, its
effects too. Sprites and fragments are rectangles, so a synthesized italic (a font without
an italic face) draws upright.

## A particle per glyph (sprites)

1. Make a Niagara System with one empty emitter. Emitter Properties: **CPU** simulation,
   **Local Space** off, and **Fixed Bounds** big enough for the text and its flight.
2. Add the user parameters above.
3. Emitter Spawn / Update: **Emitter State** with Loop Behavior **Once**; **Spawn Burst
   Instantaneous** with Spawn Count bound to `User.GlyphuxCount`.
4. Particle Spawn: **Initialize Particle** (Lifetime 2 to 3 seconds, Color white), then a
   Scratch Pad module, *Read Glyphux Glyph*, that reads entry *I* (the **Execution Index**
   node) of each array with its Get Value function and sets:
   * `Particles.Position` from `GlyphuxPositions`;
   * `Particles.SpriteSize` from `GlyphuxSizes`;
   * `Particles.DynamicMaterialParameter` to (quad, 0, 0, 0), the quad from `GlyphuxQuads`;
   * `Particles.SpriteFacing` to `GlyphuxRotation` rotating (1, 0, 0), and
     `Particles.SpriteAlignment` to it rotating (0, 0, 1);
   * a Vector attribute `Particles.GlyphCenter` from `GlyphuxCenters`, for the motion.
5. Motion: **Add Velocity from Point** with its origin bound to `Particles.GlyphCenter` for
   a burst (or a random cone, or Curl Noise), **Gravity Force** and **Drag**, then **Solve
   Forces and Velocity**; **Scale Color** to fade the alpha out over the life.
6. **Sprite Renderer**: Material User Param Binding `User.GlyphuxMaterial` (its Material
   `M_GlyphuxGlyph`, for the preview); Facing Mode **Custom Facing Vector**, Alignment
   **Custom Alignment**, so a sprite starts where its glyph was, as it was. Dynamic Material
   Binding stays `Particles.DynamicMaterialParameter`.

To assemble a text instead, spawn the particles away and move them to their positions
(**Point Attraction Force** toward `Particles.Position` saved at spawn, or a lerp in a
Scratch Pad module by normalized age).

## Fragments that shatter (meshes)

The same system with a **Mesh Renderer** instead, and a shard set: flat meshes that tile a
glyph's square, each textured by where it was in it. `Content/Python/glyphux_shards.py` makes
one, `/Glyphux/Shards/DA_GlyphuxShards`, of nine fragments (`SM_GlyphuxShard_00` and on); its
header says how to change the pattern, and `GlyphuxShardSet.h` how to model your own (thick
pieces, other shapes: any mesh whose texture coordinates are its position in the square).

1. Pass the shard set to Set Niagara Glyphs: it makes one entry per fragment of each glyph,
   positioned where the fragment sits in its glyph, with `GlyphuxMeshIndices` naming it.
2. In *Read Glyphux Glyph*, set instead of the sprite attributes:
   * `Particles.MeshIndex` from `GlyphuxMeshIndices`;
   * `Particles.MeshOrientation` to `GlyphuxRotation`;
   * `Particles.Scale` to (1, width, height), from `GlyphuxSizes` (a fragment's square is
     one unit across).
3. Motion: as for sprites, with **Add Velocity from Point** from `Particles.GlyphCenter`, so
   a glyph's pieces fly apart from its center; add spin with **Initialize Mesh
   Orientation** (a random Rotation Rate) and **Update Mesh Orientation**.
4. **Mesh Renderer**: Meshes, the shard set's in its order (`SM_GlyphuxShard_00`, `_01`, and
   on); Mesh Index Binding `Particles.MeshIndex`; **Override Materials** on, with one entry
   whose User Param Binding is `User.GlyphuxMaterial`. Facing Mode **Default**.

Hide the text while its glyphs fly (its component's **Set Draw Text** off) and show it again
after: it keeps laying out without drawing. The demo actor (`examples/unreal/Source`) does
this every few seconds, with any system built as above.

## Without Niagara

**Get Glyphs** on the text component gives each glyph's world position, size, rotation and
quad index. Spawn anything per glyph with it: a mesh with a Material Instance Dynamic of
`M_GlyphuxGlyph` (from **Get Glyph Material**) whose `GlyphuxQuad` is set through the custom
primitive data instead of the dynamic parameter (use `MF_GlyphuxGlyph` with its Quad input
fed by a Custom Primitive Data node), or physics bodies that carry the fragments.
