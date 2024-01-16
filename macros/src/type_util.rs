use proc_macro_error::emit_error;
use std::mem::take;
use syn::parse_quote_spanned;
use syn::spanned::Spanned;
use syn::GenericArgument;
use syn::PathSegment;
use syn::Token;
use syn::Type;
use tap::Pipe;

pub(crate) fn type_path_as_mono_seg(ty: &Type) -> Option<&syn::PathSegment> {
    match ty {
        Type::Path(pat) => {
            if pat.qself.is_some() {
                return None;
            }

            path_as_mono_seg(&pat.path)
        }
        _ => None,
    }
}

pub(crate) fn path_as_mono_seg(path: &syn::Path) -> Option<&syn::PathSegment> {
    if path.leading_colon.is_some() {
        return None;
    }

    if path.segments.len() != 1 {
        return None;
    }

    path.segments.first()
}

pub(crate) fn has_any_lifetime(ty: &Type) -> bool {
    match ty {
        Type::Array(x) => has_any_lifetime(&x.elem),
        Type::BareFn(_) => false,
        Type::Group(x) => has_any_lifetime(&x.elem),
        Type::ImplTrait(_) => false,
        Type::Infer(_) => false,
        Type::Macro(_) => false,
        Type::Never(_) => false,
        Type::Paren(x) => has_any_lifetime(&x.elem),
        Type::Path(x) => {
            if let Some(qs) = &x.qself {
                has_any_lifetime(&qs.ty)
            } else {
                x.path.segments.iter().any(|x| {
                    if let syn::PathArguments::AngleBracketed(args) = &x.arguments {
                        args.args
                            .iter()
                            .any(|x| matches!(x, GenericArgument::Lifetime(_)))
                    } else {
                        false
                    }
                })
            }
        }
        Type::Ptr(_) => false,
        Type::Reference(_) => true,
        Type::Slice(x) => has_any_lifetime(&x.elem),
        Type::TraitObject(x) => x.bounds.iter().any(|x| match x {
            syn::TypeParamBound::Trait(x) => {
                if let Some(lf) = &x.lifetimes {
                    lf.lifetimes
                        .iter()
                        .any(|x| matches!(x, syn::GenericParam::Lifetime(_)))
                } else {
                    false
                }
            }
            syn::TypeParamBound::Lifetime(_) => true,
            _ => false,
        }),
        Type::Tuple(x) => x.elems.iter().any(has_any_lifetime),
        Type::Verbatim(_) => false,
        _ => false,
    }
}

/// Retrieve serialization/deserialization types for given type.
pub(crate) fn retr_ser_de_params(ty: &Type) -> Option<(Type, Type)> {
    //
    // - Lifetime parameter handling; for non-static lifetimes.
    // - '__' type handling

    // 1. Elide lifetime for serialization
    // 2. Append reference for serialization

    // ---

    // Detect if type starts with `__<>`, which separates serialization/deserialization types.
    let split_ser_de_type = ty
        .pipe(|x| {
            if let Type::Path(syn::TypePath { path, .. }) = x {
                Some(path)
            } else {
                None
            }
        })
        .and_then(|x| x.segments.first().filter(|_| x.segments.len() == 1))
        .filter(|x| x.ident == "__")
        .and_then(|x| {
            if let syn::PathArguments::AngleBracketed(generics) = &x.arguments {
                if generics.args.len() != 2 {
                    emit_error!(generics, "For '__' generic type ... Expected 2 arguments");
                    return None;
                }

                fn retr_type(x: &GenericArgument) -> Option<&Type> {
                    if let GenericArgument::Type(ty) = x {
                        Some(ty)
                    } else {
                        emit_error!(x, "Non-type generic is not allowed");
                        None
                    }
                }

                Some((retr_type(&generics.args[0])?, retr_type(&generics.args[1])?))
            } else {
                None
            }
        });

    let (ty_ser, ty_de) = split_ser_de_type.unwrap_or((ty, ty));
    let [mut ty_ser, mut ty_de] = [ty_ser, ty_de].map(Clone::clone);

    let life_ser: syn::Lifetime = parse_quote_spanned! { ty_ser.span() => '___ser };
    let life_de: syn::Lifetime = parse_quote_spanned! { ty_de.span() => '___de };

    if let syn::Type::Reference(ty) = &mut ty_ser {
        ty.lifetime = Some(life_ser.clone());
    } else {
        ty_ser = Type::Reference(syn::TypeReference {
            and_token: Token![&](ty_ser.span()),
            lifetime: Some(life_ser.clone()),
            mutability: None,
            elem: Box::new(ty_ser),
        })
    }

    replace_lifetime_occurence(&mut ty_ser, &life_ser, true);
    replace_lifetime_occurence(&mut ty_de, &life_de, false);

    Some((ty_ser, ty_de))
}

// Replace 'EVERY' lifetime occurrences into given lifetime.
pub(crate) fn replace_lifetime_occurence(a: &mut Type, life: &syn::Lifetime, skip_static: bool) {
    fn replace_inner(a: &mut syn::Lifetime, life: &syn::Lifetime, skip_static: bool) {
        if skip_static && a.ident == "static" {
            return;
        }

        a.ident = life.ident.clone();
    }

    match a {
        Type::Array(x) => replace_lifetime_occurence(&mut x.elem, life, skip_static),
        Type::BareFn(_) => emit_error!(a, "You can't use function type here"),
        Type::Group(x) => replace_lifetime_occurence(&mut x.elem, life, skip_static),
        Type::ImplTrait(_) => emit_error!(a, "You can't use impl trait here"),
        Type::Infer(_) => emit_error!(a, "You can't use infer type here"),
        Type::Macro(_) => emit_error!(a, "You can't use macro type here"),
        Type::Never(_) => emit_error!(a, "You can't use never type here"),
        Type::Paren(x) => replace_lifetime_occurence(&mut x.elem, life, skip_static),
        Type::Ptr(x) => emit_error!(x, "You can't use pointer type here"),
        Type::Slice(x) => replace_lifetime_occurence(&mut x.elem, life, skip_static),
        Type::Verbatim(_) => emit_error!(a, "Failed to parse type"),

        Type::Tuple(tup) => tup
            .elems
            .iter_mut()
            .for_each(|x| replace_lifetime_occurence(x, life, skip_static)),

        Type::TraitObject(x) => x.bounds.iter_mut().for_each(|x| match x {
            syn::TypeParamBound::Trait(tr) => {
                if let Some(lf) = &mut tr.lifetimes {
                    lf.lifetimes.iter_mut().for_each(|x| {
                        if let syn::GenericParam::Lifetime(lf) = x {
                            replace_inner(&mut lf.lifetime, life, skip_static)
                        }
                    })
                }
            }
            syn::TypeParamBound::Lifetime(x) => replace_inner(x, life, skip_static),
            syn::TypeParamBound::Verbatim(_) => emit_error!(x, "Failed to parse type"),
            _ => (),
        }),

        Type::Reference(x) => {
            if let Some(lf) = &mut x.lifetime {
                replace_inner(lf, life, skip_static)
            } else {
                x.lifetime = Some(life.clone());
            }

            replace_lifetime_occurence(&mut x.elem, life, skip_static)
        }

        Type::Path(pat) => {
            if let Some(qs) = &mut pat.qself {
                replace_lifetime_occurence(&mut qs.ty, life, skip_static);
            }

            pat.path.segments.iter_mut().for_each(
                |syn::PathSegment {
                     ident: _,
                     arguments,
                 }| match arguments {
                    syn::PathArguments::None => (),
                    syn::PathArguments::AngleBracketed(items) => {
                        items.args.iter_mut().for_each(|x| {
                            if let GenericArgument::Lifetime(lf) = x {
                                replace_inner(lf, life, skip_static)
                            }
                        });
                    }
                    syn::PathArguments::Parenthesized(items) => {
                        items
                            .inputs
                            .iter_mut()
                            .for_each(|x| replace_lifetime_occurence(x, life, skip_static));

                        if let syn::ReturnType::Type(_, ty) = &mut items.output {
                            replace_lifetime_occurence(ty, life, skip_static);
                        }
                    }
                },
            );
        }

        _ => (),
    }
}

pub(crate) fn elevate_vis_level(mut in_vis: syn::Visibility, amount: usize) -> syn::Visibility {
    // - pub(super) -> pub(super::super)
    // - None -> pub(super)
    // - pub(in crate::...) -> absolute; as is
    // - pub(in super::...) -> pub(in super::super::...)

    if amount == 0 {
        return in_vis;
    }

    match in_vis {
        syn::Visibility::Public(_) => in_vis,
        syn::Visibility::Restricted(ref mut vis) => {
            let first_ident = &vis.path.segments.first().unwrap().ident;

            if first_ident == "crate" {
                // pub(in crate::...) -> Don't need to elevate
                return in_vis;
            }

            if vis.in_token.is_some() && vis.path.leading_colon.is_some() {
                // Absolute path
                return in_vis;
            }

            let is_first_token_self = first_ident == "self";
            let source = take(&mut vis.path.segments);
            vis.path.segments.extend(
                std::iter::repeat(PathSegment {
                    arguments: syn::PathArguments::None,
                    ident: syn::Ident::new("super", vis.span()),
                })
                .take(amount),
            );
            vis.path
                .segments
                .extend(source.into_iter().skip(is_first_token_self as usize));

            in_vis
        }
        syn::Visibility::Inherited => {
            let span = || in_vis.span();
            syn::Visibility::Restricted(syn::VisRestricted {
                pub_token: Token![pub](span()),
                paren_token: syn::token::Paren(span()),
                in_token: Some(Token![in](span())),
                path: Box::new(syn::Path {
                    leading_colon: None,
                    segments: std::iter::repeat(syn::PathSegment {
                        arguments: syn::PathArguments::None,
                        ident: syn::Ident::new("super", span()),
                    })
                    .take(amount)
                    .collect(),
                }),
            })
        }
    }
}

pub(crate) fn expr_into_lit_str(expr: syn::Expr) -> Option<syn::LitStr> {
    match expr {
        syn::Expr::Lit(expr) => match expr.lit {
            syn::Lit::Str(lit) => return Some(lit),
            _ => proc_macro_error::emit_error!(expr, "Expected string literal"),
        },
        _ => proc_macro_error::emit_error!(expr, "Expected string literal"),
    }

    None
}
