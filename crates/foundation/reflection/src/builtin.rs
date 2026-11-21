//! [`Reflect`] for the built-in types: primitives, `String`, `Vec`, `Option`, fixed arrays and
//! the `oxreal-math` types (ADR-0021 §2 and §3).
//!
//! They're hand-written here, not derived, because the types belong to `std` and `oxreal-math`
//! (orphan rule), and `oxreal-math` must never depend on this crate.

use oxreal_math::{
    Affine3A, DMat4, DQuat, DVec2, DVec3, DVec4, IVec2, IVec3, IVec4, LinearRgba, Mat2, Mat3,
    Mat3A, Mat4, Quat, Transform, UVec2, UVec3, UVec4, Vec2, Vec3, Vec3A, Vec4,
};

use crate::definition::MathComponentDefinition;
use crate::id::{ConstArg, builtin};
use crate::kind::Primitive;
use crate::ops::Operations;
use crate::{
    FieldDefinition, GenericArg, KindDefinition, Persistence, Reflect, TypeDefinition, TypeRef,
    TypeRegistry,
};

macro_rules! primitives {
    ($($ty:ty => $kind:ident, $id:ident, abi: $abi:literal;)*) => {$(
        // SAFETY: built with `builder::<$ty>`, so the layout and operations are `$ty`'s, and the
        // primitive kind is `$ty`'s own. Not transient. ABI-safe exactly when every bit pattern
        // is valid (integers and floats, not `bool`).
        unsafe impl Reflect for $ty {
            const TRANSIENT: bool = false;
            const ABI: bool = $abi;
            const DEFINITION: TypeDefinition = {
                let builder = TypeDefinition::builder::<$ty>(
                    stringify!($ty),
                    Persistence::Persisted(builtin::$id),
                    KindDefinition::primitive(Primitive::$kind),
                )
                .cloneable()
                .defaultable();
                if $abi { builder.abi().build() } else { builder.build() }
            };
        }
    )*};
}

primitives! {
    bool => Bool, BOOL, abi: false;
    i8 => I8, I8, abi: true;
    i16 => I16, I16, abi: true;
    i32 => I32, I32, abi: true;
    i64 => I64, I64, abi: true;
    u8 => U8, U8, abi: true;
    u16 => U16, U16, abi: true;
    u32 => U32, U32, abi: true;
    u64 => U64, U64, abi: true;
    f32 => F32, F32, abi: true;
    f64 => F64, F64, abi: true;
}

// SAFETY: built with `builder::<String>`; the string kind is `String`'s own. Not transient, not
// ABI-safe.
unsafe impl Reflect for String {
    const TRANSIENT: bool = false;
    const ABI: bool = false;
    const DEFINITION: TypeDefinition = TypeDefinition::builder::<String>(
        "String",
        Persistence::Persisted(builtin::STRING),
        KindDefinition::string(),
    )
    .cloneable()
    .defaultable()
    .build();
}

// SAFETY: built with `builder::<Vec<T>>`; the list kind names the element type `T`. The
// operations come from `Operations::of_vec::<T>`, which works on `Vec<T>`. Transient exactly
// when `T` is (an instantiation with a transient argument is transient); never ABI-safe.
unsafe impl<T: Reflect> Reflect for Vec<T> {
    const TRANSIENT: bool = T::TRANSIENT;
    const ABI: bool = false;
    const DEFINITION: TypeDefinition = TypeDefinition::builder::<Self>(
        "Vec",
        Persistence::Persisted(builtin::VEC),
        KindDefinition::list::<T>(),
    )
    .generic_args(&[GenericArg::Type(TypeRef::of::<T>())])
    .operations(Operations::of_vec::<T>())
    .build();
}

// SAFETY: built with `builder::<Option<T>>`; the option kind names the payload type `T`. The
// operations come from `Operations::of_option::<T>`. Transient exactly when `T` is; never
// ABI-safe.
unsafe impl<T: Reflect> Reflect for Option<T> {
    const TRANSIENT: bool = T::TRANSIENT;
    const ABI: bool = false;
    const DEFINITION: TypeDefinition = TypeDefinition::builder::<Self>(
        "Option",
        Persistence::Persisted(builtin::OPTION),
        KindDefinition::option::<T>(),
    )
    .generic_args(&[GenericArg::Type(TypeRef::of::<T>())])
    .operations(Operations::of_option::<T>())
    .build();
}

// SAFETY: built with `builder::<[T; N]>`; the array kind names the element type `T` and length
// `N`. The operations come from `Operations::of_array::<T, N>`. Transient exactly when `T` is;
// ABI-safe exactly when `T` is (an array has no padding of its own between elements).
unsafe impl<T: Reflect, const N: usize> Reflect for [T; N] {
    const TRANSIENT: bool = T::TRANSIENT;
    const ABI: bool = T::ABI;
    const DEFINITION: TypeDefinition = {
        let builder = TypeDefinition::builder::<Self>(
            "array",
            Persistence::Persisted(builtin::ARRAY),
            KindDefinition::array::<T, N>(),
        )
        .generic_args(&[
            GenericArg::Type(TypeRef::of::<T>()),
            GenericArg::Const(ConstArg::usize(N)),
        ])
        .operations(Operations::of_array::<T, N>());
        if T::ABI {
            builder.abi().build()
        } else {
            builder.build()
        }
    };
}

macro_rules! math {
    ($($ty:ident, $id:ident: $($component:ident: $component_ty:ty),+;)*) => {$(
        // SAFETY: built with `builder::<$ty>`, so the layout and operations are `$ty`'s. A math
        // kind has no offsets: its components are named for tools and reached through accessors.
        // Not transient; not ABI-safe (the C ABI uses its own math structs, ADR-0011 item 7).
        unsafe impl Reflect for $ty {
            const TRANSIENT: bool = false;
            const ABI: bool = false;
            const DEFINITION: TypeDefinition = TypeDefinition::builder::<$ty>(
                stringify!($ty),
                Persistence::Persisted(builtin::$id),
                KindDefinition::math(&[
                    $(MathComponentDefinition::new::<$component_ty>(stringify!($component)),)+
                ]),
            )
            .cloneable()
            .defaultable()
            .build();
        }
    )*};
}

math! {
    Vec2, VEC2: x: f32, y: f32;
    Vec3, VEC3: x: f32, y: f32, z: f32;
    Vec3A, VEC3A: x: f32, y: f32, z: f32;
    Vec4, VEC4: x: f32, y: f32, z: f32, w: f32;
    Quat, QUAT: x: f32, y: f32, z: f32, w: f32;
    Mat2, MAT2: x_axis: Vec2, y_axis: Vec2;
    Mat3, MAT3: x_axis: Vec3, y_axis: Vec3, z_axis: Vec3;
    Mat3A, MAT3A: x_axis: Vec3A, y_axis: Vec3A, z_axis: Vec3A;
    Mat4, MAT4: x_axis: Vec4, y_axis: Vec4, z_axis: Vec4, w_axis: Vec4;
    Affine3A, AFFINE3A: matrix3: Mat3A, translation: Vec3A;
    DVec2, DVEC2: x: f64, y: f64;
    DVec3, DVEC3: x: f64, y: f64, z: f64;
    DVec4, DVEC4: x: f64, y: f64, z: f64, w: f64;
    DQuat, DQUAT: x: f64, y: f64, z: f64, w: f64;
    DMat4, DMAT4: x_axis: DVec4, y_axis: DVec4, z_axis: DVec4, w_axis: DVec4;
    IVec2, IVEC2: x: i32, y: i32;
    IVec3, IVEC3: x: i32, y: i32, z: i32;
    IVec4, IVEC4: x: i32, y: i32, z: i32, w: i32;
    UVec2, UVEC2: x: u32, y: u32;
    UVec3, UVEC3: x: u32, y: u32, z: u32;
    UVec4, UVEC4: x: u32, y: u32, z: u32, w: u32;
    Transform, TRANSFORM: translation: Vec3, rotation: Quat, scale: Vec3;
}

// SAFETY: built with `builder::<LinearRgba>`; its four public `f32` fields are described with
// their real types and `offset_of!` offsets. Not transient; not ABI-safe (it isn't `repr(C)`).
unsafe impl Reflect for LinearRgba {
    const TRANSIENT: bool = false;
    const ABI: bool = false;
    const DEFINITION: TypeDefinition = TypeDefinition::builder::<LinearRgba>(
        "LinearRgba",
        Persistence::Persisted(builtin::LINEAR_RGBA),
        KindDefinition::named_struct(&[
            FieldDefinition::new::<f32>("r", std::mem::offset_of!(LinearRgba, r)),
            FieldDefinition::new::<f32>("g", std::mem::offset_of!(LinearRgba, g)),
            FieldDefinition::new::<f32>("b", std::mem::offset_of!(LinearRgba, b)),
            FieldDefinition::new::<f32>("a", std::mem::offset_of!(LinearRgba, a)),
        ]),
    )
    .cloneable()
    .defaultable()
    .build();
}

/// Registers every non-generic built-in type. Generic instantiations (`Vec<f32>`, `[f32; 4]`)
/// are registered when a type that uses them is.
pub(crate) fn register_types(registry: &TypeRegistry) {
    fn register<T: Reflect>(registry: &TypeRegistry) {
        if let Err(error) = registry.register::<T>() {
            panic!("the built-in types are valid and have distinct IDs, but: {error}");
        }
    }
    register::<bool>(registry);
    register::<i8>(registry);
    register::<i16>(registry);
    register::<i32>(registry);
    register::<i64>(registry);
    register::<u8>(registry);
    register::<u16>(registry);
    register::<u32>(registry);
    register::<u64>(registry);
    register::<f32>(registry);
    register::<f64>(registry);
    register::<String>(registry);
    register::<Vec2>(registry);
    register::<Vec3>(registry);
    register::<Vec3A>(registry);
    register::<Vec4>(registry);
    register::<Quat>(registry);
    register::<Mat2>(registry);
    register::<Mat3>(registry);
    register::<Mat3A>(registry);
    register::<Mat4>(registry);
    register::<Affine3A>(registry);
    register::<DVec2>(registry);
    register::<DVec3>(registry);
    register::<DVec4>(registry);
    register::<DQuat>(registry);
    register::<DMat4>(registry);
    register::<IVec2>(registry);
    register::<IVec3>(registry);
    register::<IVec4>(registry);
    register::<UVec2>(registry);
    register::<UVec3>(registry);
    register::<UVec4>(registry);
    register::<Transform>(registry);
    register::<LinearRgba>(registry);
}
