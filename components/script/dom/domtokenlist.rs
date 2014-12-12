/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use dom::attr::{Attr, AttrHelpers};
use dom::bindings::codegen::Bindings::DOMTokenListBinding;
use dom::bindings::codegen::Bindings::DOMTokenListBinding::DOMTokenListMethods;
use dom::bindings::error::{Fallible, InvalidCharacter, Syntax};
use dom::bindings::global::Window;
use dom::bindings::js::{JS, JSRef, Temporary, OptionalRootable};
use dom::bindings::utils::{Reflector, Reflectable, reflect_dom_object};
use dom::element::{Element, AttributeHandlers};
use dom::node::window_from_node;

use servo_util::str::{DOMString, HTML_SPACE_CHARACTERS};
use string_cache::Atom;

#[dom_struct]
pub struct DOMTokenList {
    reflector_: Reflector,
    element: JS<Element>,
    local_name: Atom,
}

impl DOMTokenList {
    pub fn new_inherited(element: JSRef<Element>, local_name: Atom) -> DOMTokenList {
        DOMTokenList {
            reflector_: Reflector::new(),
            element: JS::from_rooted(element),
            local_name: local_name,
        }
    }

    pub fn new(element: JSRef<Element>, local_name: &Atom) -> Temporary<DOMTokenList> {
        let window = window_from_node(element).root();
        reflect_dom_object(box DOMTokenList::new_inherited(element, local_name.clone()),
                           Window(*window),
                           DOMTokenListBinding::Wrap)
    }
}

impl Reflectable for DOMTokenList {
    fn reflector<'a>(&'a self) -> &'a Reflector {
        &self.reflector_
    }
}

trait PrivateDOMTokenListHelpers {
    fn attribute(self) -> Option<Temporary<Attr>>;
    fn check_token_exceptions<'a>(self, token: &'a str) -> Fallible<()>;
}

impl<'a> PrivateDOMTokenListHelpers for JSRef<'a, DOMTokenList> {
    fn attribute(self) -> Option<Temporary<Attr>> {
        let element = self.element.root();
        element.get_attribute(ns!(""), &self.local_name)
    }

    fn check_token_exceptions<'a>(self, token: &'a str) -> Fallible<()> {
        match token {
            "" => Err(Syntax),
            slice if slice.find(HTML_SPACE_CHARACTERS).is_some() => Err(InvalidCharacter),
            _ => Ok(())
        }
    }
}

// http://dom.spec.whatwg.org/#domtokenlist
impl<'a> DOMTokenListMethods for JSRef<'a, DOMTokenList> {
    // http://dom.spec.whatwg.org/#dom-domtokenlist-length
    fn Length(self) -> u32 {
        self.attribute().root().map(|attr| {
            attr.value().tokens().map(|tokens| tokens.len()).unwrap_or(0)
        }).unwrap_or(0) as u32
    }

    // http://dom.spec.whatwg.org/#dom-domtokenlist-item
    fn Item(self, index: u32) -> Option<DOMString> {
        self.attribute().root().and_then(|attr| attr.value().tokens().and_then(|tokens| {
            tokens.get(index as uint).map(|token| token.as_slice().to_string())
        }))
    }

    fn IndexedGetter(self, index: u32, found: &mut bool) -> Option<DOMString> {
        let item = self.Item(index);
        *found = item.is_some();
        item
    }

    // http://dom.spec.whatwg.org/#dom-domtokenlist-contains
    fn Contains(self, token: DOMString) -> Fallible<bool> {
        let token = Atom::from_slice(token.as_slice());
        self.check_token_exceptions(token.as_slice()).map(|()| {
            self.attribute().root().map(|attr| {
                attr.value()
                    .tokens()
                    .expect("Should have parsed this attribute")
                    .iter()
                    .any(|atom| *atom == token)
            }).unwrap_or(false)
        })
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-add
    fn Add(self, tokens: Vec<DOMString>) -> Fallible<()> {
        match self.attribute().root() {
            Some(attr) => {
                let mut atoms: Vec<Atom> = attr.value().tokens().expect("Should have parsed this attribute")
                                               .iter().map(|atom| atom.clone()).collect();

                for token in tokens.iter().map(|token| Atom::from_slice(token.as_slice())) {
                    let check = self.check_token_exceptions(token.as_slice()).map(|()| {
                        if !atoms.iter().any(|atom| *atom == token) {
                            atoms.push(token.clone());
                        }
                    });
                    if check.is_err() {
                        return Err(check.unwrap_err());
                    }
                }

                let mut tokenlist = String::new();
                for token in atoms.iter() {
                    if !tokenlist.is_empty() {
                        tokenlist.push('\u0020');
                    }
                    tokenlist.push_str(token.as_slice());
                }

                let element = self.element.root();
                element.set_tokenlist_attribute(&self.local_name, tokenlist);
            },
            None => ()
        }
        Ok(())
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-remove
    fn Remove(self, tokens: Vec<DOMString>) -> Fallible<()> {
        match self.attribute().root() {
            Some(attr) => {
                let mut atoms: Vec<Atom> = attr.value().tokens().expect("Should have parsed this attribute")
                                               .iter().map(|atom| atom.clone()).collect();

                for token in tokens.iter().map(|token| Atom::from_slice(token.as_slice())) {
                    let check = self.check_token_exceptions(token.as_slice()).map(|()| {
                        atoms.iter().position(|atom| *atom == token).and_then(|index| {
                            atoms.remove(index)
                        });
                    });
                    if check.is_err() {
                        return Err(check.unwrap_err());
                    }
                }

                let mut tokenlist = String::new();
                for token in atoms.iter() {
                    if !tokenlist.is_empty() {
                        tokenlist.push('\u0020');
                    }
                    tokenlist.push_str(token.as_slice());
                }

                let element = self.element.root();
                element.set_tokenlist_attribute(&self.local_name, tokenlist);
            },
            None => ()
        }
        Ok(())
    }

    // https://dom.spec.whatwg.org/#dom-domtokenlist-toggle
    fn Toggle(self, token: DOMString, force: Option<bool>) -> Fallible<bool> {
        let mut res = false;
        match self.attribute().root() {
            Some(attr) => {
                let mut atoms: Vec<Atom> = attr.value().tokens().expect("Should have parsed this attribute")
                                               .iter().map(|atom| atom.clone()).collect();

                let token = Atom::from_slice(token.as_slice());
                match self.check_token_exceptions(token.as_slice()) {
                    Ok(_) => match atoms.iter().position(|atom| *atom == token) {
                        Some(index) => {
                            if force.is_some() && force.unwrap() == true {
                                return Ok(true);
                            }
                            atoms.remove(index);
                            res = false;
                        },
                        None => {
                            if force.is_some() && force.unwrap() == false {
                                return Ok(false);
                            }
                            atoms.push(token);
                            res = true;
                        }
                    },
                    Err(error) => { return Err(error); }
                }

                let mut tokenlist = String::new();
                for token in atoms.iter() {
                    if !tokenlist.is_empty() {
                        tokenlist.push('\u0020');
                    }
                    tokenlist.push_str(token.as_slice());
                }

                let element = self.element.root();
                element.set_tokenlist_attribute(&self.local_name, tokenlist);
            },
            None => ()
        }

        Ok(res)
    }
}
