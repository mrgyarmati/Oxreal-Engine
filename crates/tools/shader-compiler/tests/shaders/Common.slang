// Shared helpers for the shader-compiler tests: exercises `import` and dependency tracking.
module Common;

public static const float kPi = 3.14159265;

/// A fully saturated color for `hue` in [0, 1).
public float3 hueToRgb(float hue)
{
    float3 k = float3(0.0, 2.0 / 3.0, 1.0 / 3.0);
    return saturate(abs(frac(hue + k) * 6.0 - 3.0) - 1.0);
}
