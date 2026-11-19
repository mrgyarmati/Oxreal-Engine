- Depth buffers: `TextureFormat::Depth32Float` with `TextureUsage::DEPTH_TARGET`, a depth
  attachment on render passes (`DepthAttachment`, `DepthLoadOp`), and depth testing on pipelines
  (`DepthState`, `CompareOp`). `DepthState::SCENE` is Oxreal's reversed-depth convention.
- Depth-only render passes and pipelines, for depth prepasses.
- `Frame::backbuffer_size`, for sizing depth buffers and the camera's aspect ratio.
- The `cube` example: a spinning textured cube seen through a 3D camera with a depth buffer.
