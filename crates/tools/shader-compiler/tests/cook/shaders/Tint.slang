// The cook tests' shader. It imports a module from another directory, as the renderer's shaders
// import oxreal-rhi's Bindless.slang. Do not change it without updating the golden source key in
// src/cook/key.rs.
import Palette;

struct VertexOutput
{
    float4 position : SV_Position;
    float3 color : COLOR0;
};

[shader("vertex")]
VertexOutput tintVertex(uint vertexId : SV_VulkanVertexID)
{
    const float2 corners[3] = { float2(0.0, 0.6), float2(-0.6, -0.4), float2(0.6, -0.4) };
    VertexOutput output;
    output.position = float4(corners[vertexId], 0.5, 1.0);
    output.color = paletteColor(vertexId);
    return output;
}

[shader("fragment")]
float4 tintPixel(VertexOutput input) : SV_Target
{
    return float4(input.color, 1.0);
}
