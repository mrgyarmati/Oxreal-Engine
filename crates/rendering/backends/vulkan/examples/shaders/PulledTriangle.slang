// A triangle whose vertices come from a GPU buffer (examples/triangle.rs, tests/buffers.rs).
//
// No vertex input state: the vertex shader reads ("pulls") its vertex from a buffer through the
// address passed in the push constants (ADR-0016 §2). With scalarBlockLayout (a required
// feature) the buffer uses the same C-like layout as a Rust #[repr(C)] struct:
// `struct Vertex { position: [f32; 2], color: [f32; 4] }`, 24 bytes, color at offset 8.

struct Vertex
{
    float2 position; // Clip space, Y up (ADR-0011).
    float4 color;
};

struct PulledConstants
{
    Vertex* vertices; // From Device::buffer_address. 8 bytes.
    float rotation;   // Radians, counter-clockwise.
    float aspect;     // Target width / height.
};

[[vk::push_constant]]
ConstantBuffer<PulledConstants> constants;

struct VertexOutput
{
    float4 position : SV_Position;
    float4 color : COLOR0;
};

// SV_VulkanVertexID, not SV_VertexID (coding-standards.md, Slang).
[shader("vertex")]
VertexOutput pulledVertex(uint vertexId : SV_VulkanVertexID)
{
    Vertex v = constants.vertices[vertexId];
    float s = sin(constants.rotation);
    float c = cos(constants.rotation);
    float2 p = float2(v.position.x * c - v.position.y * s, v.position.x * s + v.position.y * c);

    VertexOutput output;
    output.position = float4(p.x / constants.aspect, p.y, 0.5, 1.0);
    output.color = v.color;
    return output;
}

[shader("pixel")]
float4 pulledPixel(VertexOutput input) : SV_Target
{
    return input.color;
}
