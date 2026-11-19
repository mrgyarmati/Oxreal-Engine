// SV_VertexID follows D3D rules (excludes the base vertex), so Slang subtracts Vulkan's base
// vertex, which needs the DrawParameters capability. Oxreal doesn't require it, so this must be
// rejected. Use SV_VulkanVertexID instead.
[shader("vertex")]
float4 usesSvVertexIdVertex(uint vertexId : SV_VertexID) : SV_Position
{
    return float4(float(vertexId), 0.0, 0.5, 1.0);
}
