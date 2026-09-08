//! Entrypoint generation for rstiny-runtime.
use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, ReturnType, Type, parse_macro_input};

/// Mark `fn main(info: &mut rstiny_runtime::BootInfo) -> !` as the task entry.
/// Use once per executable; no attribute arguments are accepted.
#[proc_macro_attribute]
pub fn entry(args: TokenStream, item: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[entry] takes no arguments",
        )
        .to_compile_error()
        .into();
    }
    let function = parse_macro_input!(item as ItemFn);
    expand(function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand(function: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
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
        || sig.inputs.len() != 1
        || !argument_valid
        || !returns_never
    {
        return Err(syn::Error::new_spanned(
            sig,
            "entry must be a safe, non-generic Rust function: fn(&mut BootInfo) -> !",
        ));
    }
    let name = &sig.ident;
    let cfg = function
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"));
    Ok(quote! {
        #function

        #(#cfg)*
        const _: () = {
            unsafe extern "C" { static __user_stack_top: u8; }
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
                unsafe { ::rstiny_runtime::start(pointer, main) }
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
    fn rejects_invalid_signatures() {
        for source in [
            "async fn main(info: &mut BootInfo) -> ! { loop {} }",
            "unsafe fn main(info: &mut BootInfo) -> ! { loop {} }",
            "const fn main(info: &mut BootInfo) -> ! { loop {} }",
            "extern \"C\" fn main(info: &mut BootInfo) -> ! { loop {} }",
            "fn main<T>(info: &mut BootInfo) -> ! { loop {} }",
            "fn main() -> ! { loop {} }",
            "fn main(info: &BootInfo) -> ! { loop {} }",
            "fn main(info: &mut BootInfo) {}",
        ] {
            assert!(expand(syn::parse_str(source).unwrap()).is_err(), "{source}");
        }
    }
}
