// The first triangle (examples/triangle.rs and tests/triangle.rs).
//
// No vertex buffer: positions come from the vertex index. The corners wind counter-clockwise in
// Oxreal's Y-up clip space (ADR-0011), so the triangle is front-facing and survives back-face
// culling. Each corner has a pure color, so a readback test can tell which one ended up where:
// red apex at the top, green bottom-left, blue bottom-right.

struct TriangleConstants
{
    float rotation; // Radians, counter-clockwise.
    float aspect;   // Target width / height, keeps the triangle undistorted.
};

[[vk::push_constant]]
ConstantBuffer<TriangleConstants> constants;

struct VertexOutput
{
    float4 position : SV_Position;
    float3 color : COLOR0;
};

static const float2 kCorners[3] = { float2(0.0, 0.5), float2(-0.5, -0.5), float2(0.5, -0.5) };
static const float3 kColors[3] = { float3(1.0, 0.0, 0.0), float3(0.0, 1.0, 0.0), float3(0.0, 0.0, 1.0) };

// SV_VulkanVertexID, not SV_VertexID (coding-standards.md, Slang).
[shader("vertex")]
VertexOutput triangleVertex(uint vertexId : SV_VulkanVertexID)
{
    float2 p = kCorners[vertexId];
    float s = sin(constants.rotation);
    float c = cos(constants.rotation);
    float2 rotated = float2(p.x * c - p.y * s, p.x * s + p.y * c);

    VertexOutput output;
    output.position = float4(rotated.x / constants.aspect, rotated.y, 0.5, 1.0);
    output.color = kColors[vertexId];
    return output;
}

[shader("pixel")]
float4 trianglePixel(VertexOutput input) : SV_Target
{
    return float4(input.color, 1.0);
}
