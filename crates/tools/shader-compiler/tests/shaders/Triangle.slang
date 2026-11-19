// A vertex-buffer-free triangle: positions come from the vertex index.
// SV_VulkanVertexID, not SV_VertexID: the latter subtracts the base vertex, which needs the
// DrawParameters capability that Oxreal does not require (coding-standards.md, Slang).
import Common;

struct TriangleConstants
{
    float rotation; // radians
};

[[vk::push_constant]]
ConstantBuffer<TriangleConstants> constants;

struct VertexOutput
{
    float4 position : SV_Position;
    float3 color : COLOR0;
};

[shader("vertex")]
VertexOutput triangleVertex(uint vertexId : SV_VulkanVertexID)
{
    // Clip space is Y up (ADR-0011).
    const float2 corners[3] = { float2(0.0, 0.6), float2(-0.6, -0.4), float2(0.6, -0.4) };
    float angle = constants.rotation;
    float2 p = corners[vertexId];
    float2 rotated = float2(p.x * cos(angle) - p.y * sin(angle), p.x * sin(angle) + p.y * cos(angle));

    VertexOutput output;
    output.position = float4(rotated, 0.5, 1.0);
    output.color = hueToRgb(float(vertexId) / 3.0);
    return output;
}

[shader("fragment")]
float4 trianglePixel(VertexOutput input) : SV_Target
{
    return float4(input.color, 1.0);
}
