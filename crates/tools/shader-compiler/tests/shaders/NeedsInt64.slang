// Uses 64-bit integers, which need the Int64 capability. Oxreal doesn't require it, so the
// compiler must reject this shader (capability allowlist, ADR-0013).
[shader("fragment")]
float4 needsInt64Pixel(float4 position : SV_Position) : SV_Target
{
    uint64_t mixed = uint64_t(position.x) * 0x100000001ull;
    return float4(float(mixed & 0xffull) / 255.0, 0.0, 0.0, 1.0);
}
