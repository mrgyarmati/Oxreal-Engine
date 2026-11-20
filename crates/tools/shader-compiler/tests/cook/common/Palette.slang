// Imported by Tint.slang from another directory. Do not change it without updating the golden
// source key in src/cook/key.rs.
module Palette;

public float3 paletteColor(uint index)
{
    const float3 colors[3] = { float3(1.0, 0.2, 0.2), float3(0.2, 1.0, 0.2), float3(0.2, 0.2, 1.0) };
    return colors[index % 3];
}
