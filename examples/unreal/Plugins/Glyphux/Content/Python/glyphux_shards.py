"""Creates a Glyphux shard set: flat fragments that tile a glyph's square, for Niagara
emitters that break text apart (NIAGARA.md).

Run it in the editor like glyphux_materials.py (Tools > Execute Python Script, or
`py glyphux_shards.py` in the console). It writes, into /Glyphux/Shards:

* SM_GlyphuxShard_00 and on: one flat convex fragment each, pivot at its center, in the YZ
  plane facing +X, one unit across the square (layout x along -Y, layout y along -Z, as the
  text component lays text out), texture coordinates 0 its position in the square;
* DA_GlyphuxShards: the shard set (UGlyphuxShardSet) listing them with their centers, in the
  order the Niagara mesh renderer must list the meshes.

The fragments are the Voronoi cells of jittered points, so they are about the same size.
Change COUNT_X, COUNT_Y or SEED below and run it again for another pattern (remove the old
meshes first if there are fewer now).
"""

import random

import unreal

FOLDER = "/Glyphux/Shards"
COUNT_X = 3
COUNT_Y = 3
JITTER = 0.8
SEED = 7

ASSETS = unreal.EditorAssetLibrary


def sites():
    """Jittered points, one per cell of a COUNT_X by COUNT_Y grid over the unit square."""
    rng = random.Random(SEED)
    out = []
    for j in range(COUNT_Y):
        for i in range(COUNT_X):
            u = (i + 0.5 + JITTER * (rng.random() - 0.5)) / COUNT_X
            v = (j + 0.5 + JITTER * (rng.random() - 0.5)) / COUNT_Y
            out.append((u, v))
    return out


def clip(polygon, a, b):
    """polygon's part closer to site a than to site b (Sutherland-Hodgman, one half plane)."""
    mx, my = (a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5
    nx, ny = b[0] - a[0], b[1] - a[1]

    def side(p):
        return (p[0] - mx) * nx + (p[1] - my) * ny

    out = []
    for k, p in enumerate(polygon):
        q = polygon[(k + 1) % len(polygon)]
        sp, sq = side(p), side(q)
        if sp <= 0.0:
            out.append(p)
        if (sp < 0.0) != (sq < 0.0) and sp != sq:
            t = sp / (sp - sq)
            out.append((p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])))
    return out


def cells(points):
    """The Voronoi cell of each point, clipped to the unit square: convex polygons."""
    square = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
    out = []
    for a in points:
        polygon = square
        for b in points:
            if b is not a:
                polygon = clip(polygon, a, b)
        out.append(polygon)
    return out


def centroid(polygon):
    """The area centroid of a convex polygon."""
    area = cx = cy = 0.0
    for k, p in enumerate(polygon):
        q = polygon[(k + 1) % len(polygon)]
        cross = p[0] * q[1] - q[0] * p[1]
        area += cross
        cx += (p[0] + q[0]) * cross
        cy += (p[1] + q[1]) * cross
    if abs(area) < 1e-12:
        return polygon[0]
    return cx / (3.0 * area), cy / (3.0 * area)


def mesh(name, polygon, center):
    """A static mesh of polygon (square coordinates), pivot at center."""
    path = FOLDER + "/" + name
    if ASSETS.does_asset_exist(path):
        asset = ASSETS.load_asset(path)
    else:
        asset = unreal.AssetToolsHelpers.get_asset_tools().create_asset(name, FOLDER, unreal.StaticMesh, None)
    description = unreal.StaticMesh.create_static_mesh_description(asset)
    group = description.create_polygon_group()
    description.set_polygon_group_material_slot_name(group, "Glyph")
    vertices = []
    for u, v in polygon:
        vertex = description.create_vertex()
        # The square's u along -Y, v along -Z, from the fragment's center; flat in X.
        description.set_vertex_position(vertex, unreal.Vector(0.0, -(u - center[0]), -(v - center[1])))
        vertices.append(vertex)

    def instance(k):
        i = description.create_vertex_instance(vertices[k])
        description.set_vertex_instance_uv(i, unreal.Vector2D(polygon[k][0], polygon[k][1]), 0)
        return i

    # A fan, wound to face +X (the polygon runs clockwise seen from +X: u is -Y, v is -Z).
    for k in range(1, len(polygon) - 1):
        description.create_triangle(group, [instance(0), instance(k + 1), instance(k)])
    # The slot first, so the build finds its section's material.
    material = unreal.load_asset("/Glyphux/Materials/M_GlyphuxGlyph")
    slot = unreal.StaticMaterial(material_interface=material, material_slot_name="Glyph")
    asset.set_editor_property("static_materials", [slot])
    asset.build_from_static_mesh_descriptions([description])
    ASSETS.save_loaded_asset(asset)
    return asset


def shard_set(shards):
    path = FOLDER + "/DA_GlyphuxShards"
    if ASSETS.does_asset_exist(path):
        asset = ASSETS.load_asset(path)
    else:
        factory = unreal.DataAssetFactory()
        factory.set_editor_property("data_asset_class", unreal.GlyphuxShardSet)
        asset = unreal.AssetToolsHelpers.get_asset_tools().create_asset("DA_GlyphuxShards", FOLDER, unreal.GlyphuxShardSet, factory)
    entries = []
    for m, center in shards:
        s = unreal.GlyphuxShard()
        s.set_editor_property("mesh", m)
        s.set_editor_property("center", unreal.Vector2D(center[0], center[1]))
        entries.append(s)
    asset.set_editor_property("shards", entries)
    ASSETS.save_loaded_asset(asset)
    unreal.log("Glyphux: " + asset.get_path_name() + " (" + str(len(entries)) + " fragments)")


def main():
    shards = []
    for k, polygon in enumerate(cells(sites())):
        if len(polygon) < 3:
            continue
        center = centroid(polygon)
        shards.append((mesh("SM_GlyphuxShard_%02d" % k, polygon, center), center))
    shard_set(shards)


main()
