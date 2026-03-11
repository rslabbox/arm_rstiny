use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Error, Fields, ItemStruct, Type, parse_macro_input};

#[proc_macro_attribute]
pub fn capability_provider(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return Error::new(proc_macro2::Span::call_site(), "capability_provider takes no arguments")
            .to_compile_error()
            .into();
    }

    let item_struct = parse_macro_input!(item as ItemStruct);

    let struct_ident = item_struct.ident.clone();
    let vis = item_struct.vis.clone();
    let handle_ident = format_ident!("{}Handle", struct_ident);

    let Fields::Named(fields) = &item_struct.fields else {
        return Error::new_spanned(&item_struct, "capability_provider requires named fields")
            .to_compile_error()
            .into();
    };

    let methods = fields.named.iter().filter_map(|field| {
        let field_ident = field.ident.clone()?;
        let Type::BareFn(bare_fn) = &field.ty else {
            return None;
        };

        let mut arg_idents = Vec::new();
        let mut arg_defs = Vec::new();
        for (idx, input) in bare_fn.inputs.iter().enumerate() {
            let arg_ident = input
                .name
                .as_ref()
                .map(|(ident, _)| ident.clone())
                .unwrap_or_else(|| format_ident!("arg{idx}"));
            let arg_ty = &input.ty;
            arg_defs.push(quote! { #arg_ident: #arg_ty });
            arg_idents.push(arg_ident);
        }
        let output = &bare_fn.output;

        Some(quote! {
            pub fn #field_ident(&self, #(#arg_defs),*) #output {
                (self.ops.#field_ident)(#(#arg_idents),*)
            }
        })
    });

    let expanded = quote! {
        #item_struct

        #[derive(Clone, Copy)]
        #vis struct #handle_ident {
            ops: #struct_ident,
        }

        impl #handle_ident {
            #(#methods)*
        }

        impl crate::device::capability::CapabilityProvider for #struct_ident {
            type Handle = #handle_ident;

            fn resolve() -> Self::Handle {
                let ops = crate::device::capability::PROVIDERS
                    .iter()
                    .copied()
                    .filter_map(|provider| {
                        provider
                            .provider
                            .as_any()
                            .downcast_ref::<#struct_ident>()
                            .copied()
                            .map(|ops| (provider.priority, ops))
                    })
                    .max_by_key(|(priority, _)| *priority)
                    .map(|(_, ops)| ops)
                    .expect(concat!("no ", stringify!(#struct_ident), " registered"));

                #handle_ident { ops }
            }
        }
    };

    expanded.into()
}