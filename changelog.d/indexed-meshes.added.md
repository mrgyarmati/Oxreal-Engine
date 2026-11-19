- Indexed draws: `BufferUsage::INDEX`, `IndexFormat` (16- and 32-bit), and
  `RenderPass::set_index_buffer` and `RenderPass::draw_indexed`, with index-buffer offsets, first
  indices and vertex offsets. Index ranges that don't fit the bound buffer are refused with a
  reason.
