// A unit cube, textured on every face (examples/cube.rs, tests/depth.rs).
//
// The 36 vertices come from the vertex index, so there's no vertex buffer. In object space the
// cube spans [-0.5, 0.5] on each axis. Faces wind counter-clockwise seen from outside
// (ADR-0011), so back-face culling keeps the visible ones. Each face shows the whole texture
// upright: its first row of data along the face's top edge.
import Bindless;

struct CubeConstants
{
    // Object space to clip space: projection * view * model. Column-major, as glam stores it
    // and as oxreal-shader-compiler compiles (ADR-0011, ADR-0013).
    float4x4 clipFromObject;
    uint texture; // Device::texture_index
    uint sampler; // Device::sampler_index
};

[[vk::push_constant]]
ConstantBuffer<CubeConstants> constants;

struct VertexOutput
{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    float shade : TEXCOORD1;
};

// Each face's outward normal, and the direction of its top edge as seen from outside.
static const float3 faceNormals[6] = {
    float3(1, 0, 0), float3(-1, 0, 0),
    float3(0, 1, 0), float3(0, -1, 0),
    float3(0, 0, 1), float3(0, 0, -1),
};
static const float3 faceUps[6] = {
    float3(0, 1, 0), float3(0, 1, 0),
    float3(0, 0, -1), float3(0, 0, 1),
    float3(0, 1, 0), float3(0, 1, 0),
};

// Two triangles per face, as texture coordinates of its corners. With u running right and v
// running down the face, this order is counter-clockwise seen from outside.
static const float2 cornerUvs[6] = {
    float2(0, 0), float2(0, 1), float2(1, 1),
    float2(0, 0), float2(1, 1), float2(1, 0),
};

// SV_VulkanVertexID, not SV_VertexID (coding-standards.md, Slang).
[shader("vertex")]
VertexOutput cubeVertex(uint vertexId : SV_VulkanVertexID)
{
    uint face = vertexId / 6;
    float2 uv = cornerUvs[vertexId % 6];
    float3 normal = faceNormals[face];
    float3 up = faceUps[face];
    // Seen from outside, right × up points out of the face.
    float3 right = cross(up, normal);
    float3 position = 0.5 * normal + (uv.x - 0.5) * right - (uv.y - 0.5) * up;

    VertexOutput output;
    output.position = mul(constants.clipFromObject, float4(position, 1.0));
    output.uv = uv;
    // A fixed brightness per face, so the faces stay distinguishable. It's not lighting: it
    // turns with the cube.
    output.shade = 0.6 + 0.4 * saturate(dot(normal, normalize(float3(0.3, 0.8, 0.5))));
    return output;
}

[shader("pixel")]
float4 cubePixel(VertexOutput input) : SV_Target
{
    float4 color = sampleTexture(constants.texture, constants.sampler, input.uv);
    return float4(color.rgb * input.shade, color.a);
}

// For depth-only passes (no color attachments): the depth test and write happen without any
// color output.
[shader("pixel")]
void cubeDepthOnly(VertexOutput input)
{
}
