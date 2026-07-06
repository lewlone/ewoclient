#version 330 compatibility

/* 26.2 Iris splits terrain: this is the CUTOUT half (kelp, seagrass,
   leaves, flowers — anything alpha-tested). It does NOT fall back to
   gbuffers_terrain on Iris 1.11.1, so it must be provided explicitly or
   cutout foliage renders built-in, escaping every pack feature (the
   final root of the kelp-through-water saga — program name pulled from
   Iris's ProgramId enum, not guessed). Body: lib/terrain_geometry_vsh.glsl. */

#include "/lib/settings.glsl"
#include "/lib/terrain_geometry_vsh.glsl"
