- Bindless textures and samplers (ADR-0016 §2).
  - Textures with `TextureUsage::SAMPLED` get an index in one global descriptor set
    (`Device::texture_index`), and `create_sampler` gives samplers one too.
  - Shaders sample by index through `Bindless.slang`, in `crates/rendering/rhi/shaders`.
  - Sampled textures can be sampled at any time, including right after a render pass that drew
    into them.
- `Device::write_texture` uploads pixels. Like `write_buffer`, it takes effect in submission
  order.
- `textured` example: a checkerboard texture sampled through its bindless index.
