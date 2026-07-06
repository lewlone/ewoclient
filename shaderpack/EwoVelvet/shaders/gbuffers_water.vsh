#version 330 compatibility

/* Legacy translucent route (pre-colorwheel / whatever Iris still maps
   here). Body shared with clrwl_gbuffers_translucent — the two programs
   MUST be pixel-identical. See lib/translucent_geometry_vsh.glsl. */

#include "/lib/settings.glsl"
#include "/lib/translucent_geometry_vsh.glsl"
