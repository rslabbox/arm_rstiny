//! Entrypoint generation for rstiny-runtime.
use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, ReturnType, Type, parse_macro_input};

/// Mark a root `fn(&mut BootInfo) -> !` or ordinary task `fn() -> !` entry.
/// Root entries accept `stack_size = expression` (rounded up to a page).
#[proc_macro_attribute]
pub fn entry(args: TokenStream, item: TokenStream) -> TokenStream {
    let options = parse_macro_input!(args with syn::punctuated::Punctuated::<syn::MetaNameValue, syn::Token![,]>::parse_terminated);
    let mut stack_size = None;
    for option in options {
        if !option.path.is_ident("stack_size") || stack_size.is_some() {
            return syn::Error::new_spanned(option, "expected a single stack_size = expression")
                .to_compile_error()
                .into();
        }
        stack_size = Some(option.value);
    }
    let function = parse_macro_input!(item as ItemFn);
    expand_with_stack(function, stack_size)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[cfg(test)]
fn expand(function: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    expand_with_stack(function, None)
}

fn expand_with_stack(
    function: ItemFn,
    stack_size: Option<syn::Expr>,
) -> syn::Result<proc_macro2::TokenStream> {
    let sig = &function.sig;
    let argument_valid = matches!(sig.inputs.first(), Some(syn::FnArg::Typed(arg))
        if matches!(&*arg.ty, Type::Reference(reference) if reference.mutability.is_some()));
    let returns_never =
        matches!(&sig.output, ReturnType::Type(_, ty) if matches!(&**ty, Type::Never(_)));
    if sig.constness.is_some()
        || sig.asyncness.is_some()
        || sig.unsafety.is_some()
        || sig.abi.is_some()
        || sig.variadic.is_some()
        || !sig.generics.params.is_empty()
        || sig.generics.where_clause.is_some()
        || !(sig.inputs.is_empty() || (sig.inputs.len() == 1 && argument_valid))
        || !returns_never
    {
        return Err(syn::Error::new_spanned(
            sig,
            "entry must be a safe, non-generic Rust function: fn() -> ! or fn(&mut BootInfo) -> !",
        ));
    }
    let name = &sig.ident;
    if sig.inputs.is_empty() {
        if let Some(size) = stack_size {
            return Err(syn::Error::new_spanned(
                size,
                "ordinary task stacks are supplied by the loader",
            ));
        }
        let cfg = function
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("cfg"));
        return Ok(quote! {
            #function
            #(#cfg)*
            const _: () = {
                #[unsafe(export_name = "_start")]
                #[unsafe(link_section = ".text.entry")]
                extern "C" fn entry() -> ! {
                    let main: fn() -> ! = #name;
                    main()
                }
            };
        });
    }
    let cfg: Vec<_> = function
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();
    let stack_size = stack_size.unwrap_or_else(|| syn::parse_quote!(16 * 1024));
    Ok(quote! {
        #function

        #(#cfg)*
        core::arch::global_asm!(
            ".pushsection .bss.root_stack, \"aw\", @nobits",
            ".balign 4096",
            ".global __user_stack_guard", "__user_stack_guard:", ".skip 4096",
            ".global __user_stack_bottom", "__user_stack_bottom:", ".skip {size}",
            ".global __user_stack_top", "__user_stack_top:",
            ".popsection", size = const {
                let requested: usize = #stack_size;
                assert!(requested > 0, "root stack must be nonempty");
                requested.div_ceil(4096) * 4096
            },
        );
        #(#cfg)*
        const _: () = {
            unsafe extern "C" { static __user_stack_top: u8; static __user_stack_guard: u8; }
            #[unsafe(naked)]
            #[unsafe(export_name = "_start")]
            #[unsafe(link_section = ".text.entry")]
            unsafe extern "C" fn entry(pointer: *const ()) -> ! {
                core::arch::naked_asm!("ldr x9, ={stack}", "mov sp, x9", "b {main}",
                    stack = sym __user_stack_top, main = sym trampoline);
            }
            unsafe extern "C" fn trampoline(pointer: *const ()) -> ! {
                // Type checking here also accepts aliases for BootInfo, while
                // rejecting unrelated mutable references and static borrows.
                let main: fn(&mut ::rstiny_runtime::BootInfo) -> ! = #name;
                // SAFETY: only this page-aligned, unreferenced guard is removed;
                // SP already points into the separate stack storage above it.
                unsafe {
                    ::rstiny_runtime::protect_stack(core::ptr::addr_of!(__user_stack_guard) as usize);
                    ::rstiny_runtime::start(pointer, main)
                }
            }
        };
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_entry_and_emits_valid_rust() {
        let output = expand(syn::parse_quote! {
            fn main(info: &mut BootInfo) -> ! { loop {} }
        })
        .unwrap();
        syn::parse2::<syn::File>(output).unwrap();
    }

    #[test]
    fn stack_configuration_is_root_only() {
        let ordinary = syn::parse_quote!(
            fn main() -> ! {
                loop {}
            }
        );
        assert!(expand_with_stack(ordinary, Some(syn::parse_quote!(8192))).is_err());
        let root = syn::parse_quote!(
            fn main(info: &mut BootInfo) -> ! {
                loop {}
            }
        );
        let output = expand_with_stack(root, Some(syn::parse_quote!(32 * 1024 - 1))).unwrap();
        syn::parse2::<syn::File>(output).unwrap();
    }

    #[test]
    fn rejects_invalid_signatures() {
        for source in [
            "async fn main(info: &mut BootInfo) -> ! { loop {} }",
            "unsafe fn main(info: &mut BootInfo) -> ! { loop {} }",
            "const fn main(info: &mut BootInfo) -> ! { loop {} }",
            "extern \"C\" fn main(info: &mut BootInfo) -> ! { loop {} }",
            "fn main<T>(info: &mut BootInfo) -> ! { loop {} }",
            "fn main(info: &BootInfo) -> ! { loop {} }",
            "fn main(info: &mut BootInfo) {}",
            "async fn main() -> ! { loop {} }",
            "fn main() {}",
        ] {
            assert!(expand(syn::parse_str(source).unwrap()).is_err(), "{source}");
        }
    }

    #[test]
    fn ordinary_entry_needs_no_bootinfo_or_linker_stack() {
        let output = expand(syn::parse_quote! {
            fn main() -> ! { loop {} }
        })
        .unwrap();
        let source = output.to_string();
        assert!(!source.contains("BootInfo"));
        assert!(!source.contains("__user_stack_top"));
        syn::parse2::<syn::File>(output).unwrap();
    }
}
