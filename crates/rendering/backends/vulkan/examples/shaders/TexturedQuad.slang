// A full-target quad that samples a bindless texture (tests/textures.rs, examples/textured.rs).
//
// One triangle covers the whole target. `uv` (0,0) is the target's top-left corner, so the
// texture's first row of data appears at the top: clip space is Y up (ADR-0011), so v grows
// downwards as clip-space y falls.
import Bindless;

struct QuadConstants
{
    uint texture; // Device::texture_index
    uint sampler; // Device::sampler_index
    float scale;  // Texture repeats across the target (1 = once).
};

[[vk::push_constant]]
ConstantBuffer<QuadConstants> constants;

struct VertexOutput
{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};

// SV_VulkanVertexID, not SV_VertexID (coding-standards.md, Slang).
[shader("vertex")]
VertexOutput quadVertex(uint vertexId : SV_VulkanVertexID)
{
    // (0,0), (2,0), (0,2): a triangle twice the target's size, covering it entirely.
    float2 uv = float2((vertexId << 1) & 2, vertexId & 2);
    VertexOutput output;
    output.position = float4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.5, 1.0);
    output.uv = uv;
    return output;
}

[shader("pixel")]
float4 quadPixel(VertexOutput input) : SV_Target
{
    return sampleTexture(constants.texture, constants.sampler, input.uv * constants.scale);
}
