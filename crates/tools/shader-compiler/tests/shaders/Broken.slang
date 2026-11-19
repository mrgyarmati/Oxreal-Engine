// Deliberately invalid: the compiler must report a readable error.
[shader("fragment")]
float4 brokenPixel() : SV_Target
{
    return undefinedSymbol;
}
