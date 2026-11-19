// A textured mesh drawn with indices (examples/cube.rs).
//
// The vertex shader reads ("pulls") its vertex from a buffer through the address in the push
// constants (ADR-0016 §2). With indexed draws its vertex ID is the index plus the draw's vertex
// offset, so it reads whichever vertex the index buffer names.
//
// Slang lays out what a pointer points to with its natural (C-like) layout, which
// scalarBlockLayout (a required feature) permits. So the buffer matches the Rust
// `#[repr(C)] struct MeshVertex { position: [f32; 3], normal: [f32; 3], uv: [f32; 2] }`:
// 32 bytes, normal at offset 12, uv at 24. A Slang upgrade or a compiler flag could change
// that, so tests/shader_layout.rs checks these offsets and the push constants' in the SPIR-V.
import Bindless;

struct MeshVertex
{
    float3 position; // Object space, metres (ADR-0011).
    float3 normal;   // Object space, unit length.
    float2 uv;       // Texture coordinates, (0, 0) at the texture's first texel.
};

struct MeshConstants
{
    // Object space to clip space: projection * view * model. Column-major, as glam stores it
    // and as oxreal-shader-compiler compiles (ADR-0011, ADR-0013).
    float4x4 clipFromObject;
    MeshVertex* vertices; // From Device::buffer_address. 8 bytes, at offset 64.
    uint texture;         // Device::texture_index
    uint sampler;         // Device::sampler_index
};

[[vk::push_constant]]
ConstantBuffer<MeshConstants> constants;

struct VertexOutput
{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    float shade : TEXCOORD1;
};

// SV_VulkanVertexID, not SV_VertexID (coding-standards.md, Slang).
[shader("vertex")]
VertexOutput meshVertex(uint vertexId : SV_VulkanVertexID)
{
    MeshVertex v = constants.vertices[vertexId];

    VertexOutput output;
    output.position = mul(constants.clipFromObject, float4(v.position, 1.0));
    output.uv = v.uv;
    // A fixed brightness per object-space direction, so a cube's faces stay distinguishable.
    // It's not lighting: it turns with the mesh.
    output.shade = 0.6 + 0.4 * saturate(dot(v.normal, normalize(float3(0.3, 0.8, 0.5))));
    return output;
}

[shader("pixel")]
float4 meshPixel(VertexOutput input) : SV_Target
{
    float4 color = sampleTexture(constants.texture, constants.sampler, input.uv);
    return float4(color.rgb * input.shade, color.a);
}
