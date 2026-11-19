// The bindless binding model (ADR-0016 §2), the shader side of `oxreal_rhi::BINDLESS_SET`.
//
// Every pipeline sees one global descriptor set (set 0):
//   binding 0: every texture with TextureUsage::SAMPLED, at its Device::texture_index
//   binding 1: every sampler, at its Device::sampler_index
// Keep this in sync with crates/rendering/rhi/src/bindless.rs. Indices come from push constants
// or buffers and are plain numbers: only pass indices of live textures and samplers
// (ShaderCode::from_spirv_unchecked's contract).
//
// Usage: compile with crates/rendering/rhi/shaders as an include directory, then
//   import Bindless;
//   float4 color = sampleTexture(textureIndex, samplerIndex, uv);
module Bindless;

[[vk::binding(0, 0)]]
public Texture2D gTextures[];

[[vk::binding(1, 0)]]
public SamplerState gSamplers[];

/// Samples texture `texture` with sampler `sampler` at `uv` (0,0 is the texture's top-left
/// texel, its first row in Device::write_texture's data). The indices may differ per pixel.
public float4 sampleTexture(uint texture, uint sampler, float2 uv)
{
    return gTextures[NonUniformResourceIndex(texture)].Sample(
        gSamplers[NonUniformResourceIndex(sampler)], uv);
}
