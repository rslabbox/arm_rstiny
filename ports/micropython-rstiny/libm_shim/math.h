/*
 * Minimal freestanding <math.h> for the vendored lib/libm_dbl sources only
 * (ports/micropython-rstiny Makefile, FP=1). The aarch64-linux-gnu- toolchain
 * is glibc-based and its real <math.h> declares one-argument internal aliases
 * (__cos/__sin/__tan) that collide with musl-derived libm's two/three-argument
 * helpers, so these translation units compile against this header instead.
 * Everything comes from GCC builtins plus the prototypes of the functions the
 * same libm sources define; GCC 14 treats implicit declarations as errors.
 */
#ifndef MICROPY_RSTINY_LIBM_SHIM_MATH_H
#define MICROPY_RSTINY_LIBM_SHIM_MATH_H

#define NAN        (__builtin_nan(""))
#define HUGE_VAL   (__builtin_huge_val())
#define HUGE_VALL  (__builtin_huge_vall())
#define INFINITY   (__builtin_inf())

/* C classification constants used by __fpclassify.c. */
#define FP_NAN       0
#define FP_INFINITE  1
#define FP_ZERO      2
#define FP_SUBNORMAL 3
#define FP_NORMAL    4

/* FLT_EVAL_METHOD == 0 on AArch64: float_t/double_t are exactly these. */
typedef float float_t;
typedef double double_t;

#define isnan(x)     __builtin_isnan(x)
#define isinf(x)     __builtin_isinf_sign(x)
#define isfinite(x)  __builtin_isfinite(x)
#define signbit(x)   __builtin_signbit(x)
#define fabs(x)      __builtin_fabs(x)
#define copysign(x, y) __builtin_copysign(x, y)
#define sqrt(x)      __builtin_sqrt(x)

double acos(double);
double acosh(double);
double asin(double);
double asinh(double);
double atan(double);
double atan2(double, double);
double atanh(double);
double ceil(double);
double copysign(double, double);
double cos(double);
double cosh(double);
double erf(double);
double erfc(double);
double exp(double);
double expm1(double);
double fabs(double);
double floor(double);
double fmod(double, double);
double frexp(double, int *);
double hypot(double, double);
double ldexp(double, int);
double lgamma(double);
double log(double);
double log10(double);
double log1p(double);
double modf(double, double *);
double nearbyint(double);
double pow(double, double);
double rint(double);
double round(double);
double scalbn(double, int);
double sin(double);
double sinh(double);
double sqrt(double);
double tan(double);
double tanh(double);
double tgamma(double);
double trunc(double);

#endif /* MICROPY_RSTINY_LIBM_SHIM_MATH_H */
