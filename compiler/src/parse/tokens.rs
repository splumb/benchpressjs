use crate::{
    console::warn,
    parse::{
        expression::{
            expression,
            Expression,
        },
        path::PathPart,
        ws,
        Span,
        SpanExt,
    },
};
use nom::{
    branch::alt,
    bytes::complete::{
        tag,
        take_until,
    },
    combinator::{
        consumed,
        map,
        recognize,
    },
    error::ParseError,
    sequence::{
        delimited,
        pair,
    },
    IResult,
    Slice,
};

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub enum Token<S> {
    // Template text passed through
    Text(S),
    // `{obj.prop}`
    InterpEscaped { span: S, expr: Expression<S> },
    // `{{obj.prop}}`
    InterpRaw { span: S, expr: Expression<S> },
    // `{{{ if condition }}}`
    If { span: S, subject: Expression<S> },
    // `{{{ each arr }}}`
    Each { span: S, subject: Expression<S> },
    // `{{{ else }}}`
    Else { span: S },
    // `{{{ end }}}`
    End { span: S },
    // `<!-- IF condition -->`
    LegacyIf { span: S, subject: Expression<S> },
    // `<!-- BEGIN arr -->`
    LegacyBegin { span: S, subject: Expression<S> },
    // `<!-- ELSE -->`
    LegacyElse { span: S },
    // `<!-- END -->` or `<!-- ENDIF -->` or
    // `<!-- END subject -->` or `<!-- ENDIF subject -->`
    LegacyEnd { span: S, subject_raw: S },
}

impl<'a> Token<Span<'a>> {
    pub fn span(&self) -> Span<'a> {
        match self {
            Token::Text(span) => *span,
            Token::InterpEscaped { span, .. } => *span,
            Token::InterpRaw { span, .. } => *span,
            Token::If { span, .. } => *span,
            Token::Each { span, .. } => *span,
            Token::Else { span, .. } => *span,
            Token::End { span, .. } => *span,
            Token::LegacyIf { span, .. } => *span,
            Token::LegacyBegin { span, .. } => *span,
            Token::LegacyElse { span, .. } => *span,
            Token::LegacyEnd { span, .. } => *span,
        }
    }
}

fn interp_escaped(input: Span) -> IResult<Span, Token<Span>> {
    map(
        consumed(delimited(tag("{"), ws(expression), tag("}"))),
        |(span, expr)| Token::InterpEscaped { span, expr },
    )(input)
}

fn interp_raw(input: Span) -> IResult<Span, Token<Span>> {
    map(
        consumed(delimited(tag("{{"), ws(expression), tag("}}"))),
        |(span, expr)| Token::InterpRaw { span, expr },
    )(input)
}

fn new_each(input: Span) -> IResult<Span, Token<Span>> {
    map(
        consumed(delimited(
            pair(tag("{{{"), ws(tag("each"))),
            ws(expression),
            tag("}}}"),
        )),
        |(span, subject)| Token::Each { span, subject },
    )(input)
}

fn new_if(input: Span) -> IResult<Span, Token<Span>> {
    map(
        consumed(delimited(
            pair(tag("{{{"), ws(tag("if"))),
            ws(expression),
            tag("}}}"),
        )),
        |(span, subject)| Token::If { span, subject },
    )(input)
}

fn new_else(input: Span) -> IResult<Span, Token<Span>> {
    map(
        recognize(delimited(tag("{{{"), ws(tag("else")), tag("}}}"))),
        |span| Token::Else { span },
    )(input)
}

fn new_end(input: Span) -> IResult<Span, Token<Span>> {
    map(
        recognize(delimited(tag("{{{"), ws(tag("end")), tag("}}}"))),
        |span| Token::End { span },
    )(input)
}

fn legacy_begin(input: Span) -> IResult<Span, Token<Span>> {
    map(
        consumed(delimited(
            pair(tag("<!--"), ws(tag("BEGIN"))),
            ws(expression),
            tag("-->"),
        )),
        |(span, subject)| Token::LegacyBegin { span, subject },
    )(input)
}

fn legacy_if(input: Span) -> IResult<Span, Token<Span>> {
    map(
        consumed(delimited(
            pair(tag("<!--"), ws(tag("IF"))),
            ws(expression),
            tag("-->"),
        )),
        |(span, subject)| Token::LegacyIf {
            span,
            subject: {
                // Handle legacy IF helpers being passed @root as implicit first argument
                if let Expression::LegacyHelper {
                    span,
                    name,
                    mut args,
                } = subject
                {
                    args.insert(
                        0,
                        Expression::Path {
                            span: args
                                .get(0)
                                .map_or_else(|| span.slice(span.len()..), |x| x.span().slice(..0)),
                            path: vec![PathPart::Part(Span::new_extra("@root", input.extra))],
                        },
                    );

                    Expression::LegacyHelper { span, name, args }
                } else {
                    subject
                }
            },
        },
    )(input)
}

fn legacy_else(input: Span) -> IResult<Span, Token<Span>> {
    map(
        recognize(delimited(tag("<!--"), ws(tag("ELSE")), tag("-->"))),
        |span| Token::LegacyElse { span },
    )(input)
}

fn trim_end(input: Span) -> Span {
    input.slice(..(input.trim_end().len()))
}

fn legacy_end(input: Span) -> IResult<Span, Token<Span>> {
    map(
        consumed(delimited(
            pair(tag("<!--"), ws(alt((tag("ENDIF"), tag("END"))))),
            ws(take_until("-->")),
            tag("-->"),
        )),
        |(span, subject)| Token::LegacyEnd {
            span,
            subject_raw: trim_end(subject),
        },
    )(input)
}

fn token(input: Span) -> IResult<Span, Token<Span>> {
    alt((
        interp_escaped,
        interp_raw,
        new_each,
        new_if,
        new_else,
        new_end,
        legacy_begin,
        legacy_if,
        legacy_else,
        legacy_end,
    ))(input)
}

static PATTERNS: &[&str] = &[
    "\\{{{", "\\{{", "\\{", "\\<!--", "{", "<!--", "@key", "@value", "@index",
];

use aho_corasick::{
    AhoCorasick,
    AhoCorasickBuilder,
    MatchKind,
};
lazy_static::lazy_static! {
    static ref TOKEN_START: AhoCorasick = AhoCorasickBuilder::new().auto_configure(PATTERNS).match_kind(MatchKind::LeftmostFirst).build(PATTERNS);
}

#[rustfmt::skip::macros(warn)]
pub fn tokens(mut input: Span) -> IResult<Span, Vec<Token<Span>>> {
    let mut tokens = vec![];
    let mut index = 0;

    while index < input.len() {
        // skip to the next `{` or `<!--`
        if let Some(i) = TOKEN_START.find(input.slice(index..).fragment()) {
            // If this is an opener, step to it
            if matches!(i.pattern(), 4..=5) {
                index += i.start();
            // If this is an escaped opener, skip it
            } else if matches!(i.pattern(), 0..=3) {
                let start = index + i.start();
                let length = i.end() - i.start();

                // Add text before the escaper character
                if start > 0 {
                    tokens.push(Token::Text(input.slice(..start)));
                }
                // Advance to after the escaper character
                input = input.slice((start + 1)..);
                // Step to after the escaped sequence
                index = length - 1;
                continue;
            // If this is `@key`, `@value`, `@index`
            } else {
                // if matches!(i.pattern(), 6..=8)
                let start = index + i.start();
                let end = index + i.end();
                let span = input.slice(start..end);
                let (_, expr) = expression(span)?;

                let (line, column, padding) = span.get_line_column_padding();
                warn!("[benchpress] warning: keyword outside an interpolation token is deprecated");
                warn!("     --> {}:{}:{}",
                    span.extra.filename, span.location_line(), column);
                warn!("      |");
                warn!("{:>5} | {}", span.location_line(), line);
                warn!("      | {}{} help: wrap this in curly braces: `{{{}}}`",
                    padding, "^".repeat(span.len()), span);
                warn!("      | note: This will become an error in the v3.0.0\n");

                // Add text before the token
                if start > 0 {
                    tokens.push(Token::Text(input.slice(..start)));
                }
                // Add token
                tokens.push(Token::InterpEscaped { span, expr });

                // Advance to after the token
                input = input.slice(end..);
                index = 0;
                continue;
            }
        } else {
            // no tokens found, break out
            index = input.len();
            break;
        }

        match token(input.slice(index..)) {
            // Not a match, step to the next character
            Err(nom::Err::Error(_)) => {
                // do-while
                while {
                    index += 1;
                    !input.is_char_boundary(index)
                } {}
            }
            Ok((rest, tok)) => {
                // Token returned what it was sent, this shouldn't happen
                if rest == input {
                    return Err(nom::Err::Error(nom::error::Error::from_error_kind(
                        rest,
                        nom::error::ErrorKind::SeparatedList,
                    )));
                }

                // Add text before the token
                if index > 0 {
                    tokens.push(Token::Text(input.slice(..index)));
                }
                // Add token
                tokens.push(tok);

                // Advance to after the token
                input = rest;
                index = 0;
            }
            // Pass through other errors
            Err(e) => return Err(e),
        }
    }

    if index > 0 {
        tokens.push(Token::Text(input.slice(..index)));
    }

    Ok((input.slice(input.len()..), tokens))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::parse::{
        path::PathPart,
        test::{
            assert_eq_unspan,
            sp,
        },
    };

    impl<'a> Token<Span<'a>> {
        pub fn span_to_str(self) -> Token<&'a str> {
            match self {
                Token::Text(span) => Token::Text(*span.fragment()),
                Token::InterpEscaped { span, expr } => Token::InterpEscaped {
                    span: *span.fragment(),
                    expr: expr.span_to_str(),
                },
                Token::InterpRaw { span, expr } => Token::InterpRaw {
                    span: *span.fragment(),
                    expr: expr.span_to_str(),
                },
                Token::If { span, subject } => Token::If {
                    span: *span.fragment(),
                    subject: subject.span_to_str(),
                },
                Token::Each { span, subject } => Token::Each {
                    span: *span.fragment(),
                    subject: subject.span_to_str(),
                },
                Token::Else { span } => Token::Else {
                    span: *span.fragment(),
                },
                Token::End { span } => Token::End {
                    span: *span.fragment(),
                },
                Token::LegacyIf { span, subject } => Token::LegacyIf {
                    span: *span.fragment(),
                    subject: subject.span_to_str(),
                },
                Token::LegacyBegin { span, subject } => Token::LegacyBegin {
                    span: *span.fragment(),
                    subject: subject.span_to_str(),
                },
                Token::LegacyElse { span } => Token::LegacyElse {
                    span: *span.fragment(),
                },
                Token::LegacyEnd { span, subject_raw } => Token::LegacyEnd {
                    span: *span.fragment(),
                    subject_raw: *subject_raw.fragment(),
                },
            }
        }
    }

    fn span_to_str<'a>(
        res: IResult<Span<'a>, Token<Span<'a>>>,
    ) -> IResult<&'a str, Token<&'a str>> {
        match res {
            Ok((rest, tok)) => Ok((*rest.fragment(), tok.span_to_str())),
            Err(err) => Err(
                err.map(|nom::error::Error { input, code }| nom::error::Error {
                    input: *input.fragment(),
                    code,
                }),
            ),
        }
    }

    #[test]
    fn test_interp_escaped() {
        assert_eq_unspan!(
            interp_escaped(sp("{prop}")),
            Ok((
                "",
                Token::InterpEscaped {
                    span: "{prop}",
                    expr: Expression::Path {
                        span: "prop",
                        path: vec![PathPart::Part("prop")]
                    }
                }
            ))
        );
        assert_eq_unspan!(
            interp_escaped(sp("{ call() } stuff")),
            Ok((
                " stuff",
                Token::InterpEscaped {
                    span: "{ call() }",
                    expr: Expression::Helper {
                        span: "call()",
                        name: "call",
                        args: vec![]
                    }
                }
            ))
        );
    }

    #[test]
    fn test_interp_raw() {
        assert_eq_unspan!(
            interp_raw(sp("{{prop}}")),
            Ok((
                "",
                Token::InterpRaw {
                    span: "{{prop}}",
                    expr: Expression::Path {
                        span: "prop",
                        path: vec![PathPart::Part("prop")]
                    }
                }
            ))
        );
        assert_eq_unspan!(
            interp_raw(sp("{{ call() }} stuff")),
            Ok((
                " stuff",
                Token::InterpRaw {
                    span: "{{ call() }}",
                    expr: Expression::Helper {
                        span: "call()",
                        name: "call",
                        args: vec![]
                    }
                }
            ))
        );
    }

    #[test]
    fn test_new_if() {
        assert_eq_unspan!(
            new_if(sp("{{{if abc}}}")),
            Ok((
                "",
                Token::If {
                    span: "{{{if abc}}}",
                    subject: Expression::Path {
                        span: "abc",
                        path: vec![PathPart::Part("abc")]
                    }
                }
            ))
        );
        assert_eq_unspan!(
            new_if(sp("{{{ if call() }}}")),
            Ok((
                "",
                Token::If {
                    span: "{{{ if call() }}}",
                    subject: Expression::Helper {
                        span: "call()",
                        name: "call",
                        args: vec![]
                    }
                }
            ))
        );
    }

    #[test]
    fn test_new_each() {
        assert_eq_unspan!(
            new_each(sp("{{{each abc.def}}}")),
            Ok((
                "",
                Token::Each {
                    span: "{{{each abc.def}}}",
                    subject: Expression::Path {
                        span: "abc.def",
                        path: vec![PathPart::Part("abc"), PathPart::Part("def")]
                    }
                }
            ))
        );
        assert_eq_unspan!(
            new_each(sp("{{{ each call() }}}")),
            Ok((
                "",
                Token::Each {
                    span: "{{{ each call() }}}",
                    subject: Expression::Helper {
                        span: "call()",
                        name: "call",
                        args: vec![]
                    }
                }
            ))
        );
    }

    #[test]
    fn test_new_else() {
        assert_eq_unspan!(
            new_else(sp("{{{else}}}")),
            Ok(("", Token::Else { span: "{{{else}}}" }))
        );
        assert_eq_unspan!(
            new_else(sp("{{{ else }}}")),
            Ok((
                "",
                Token::Else {
                    span: "{{{ else }}}"
                }
            ))
        );
    }

    #[test]
    fn test_new_end() {
        assert_eq_unspan!(
            new_end(sp("{{{end}}}")),
            Ok(("", Token::End { span: "{{{end}}}" }))
        );
        assert_eq_unspan!(
            new_end(sp("{{{ end }}}")),
            Ok((
                "",
                Token::End {
                    span: "{{{ end }}}"
                }
            ))
        );
    }

    #[test]
    fn test_legacy_if() {
        assert_eq_unspan!(
            legacy_if(sp("<!--IF abc-->")),
            Ok((
                "",
                Token::LegacyIf {
                    span: "<!--IF abc-->",
                    subject: Expression::Path {
                        span: "abc",
                        path: vec![PathPart::Part("abc")]
                    }
                }
            ))
        );
        assert_eq_unspan!(
            legacy_if(sp("<!-- IF call() -->")),
            Ok((
                "",
                Token::LegacyIf {
                    span: "<!-- IF call() -->",
                    subject: Expression::Helper {
                        span: "call()",
                        name: "call",
                        args: vec![]
                    }
                }
            ))
        );
        assert_eq_unspan!(
            legacy_if(sp("<!--IF function.bar, a, b -->")),
            Ok((
                "",
                Token::LegacyIf {
                    span: "<!--IF function.bar, a, b -->",
                    subject: Expression::LegacyHelper {
                        span: "function.bar, a, b",
                        name: "bar",
                        args: vec![
                            Expression::Path {
                                span: "",
                                path: vec![PathPart::Part("@root")]
                            },
                            Expression::Path {
                                span: "a",
                                path: vec![PathPart::Part("a")]
                            },
                            Expression::Path {
                                span: "b",
                                path: vec![PathPart::Part("b")]
                            },
                        ]
                    }
                }
            ))
        );
    }

    #[test]
    fn test_legacy_begin() {
        assert_eq_unspan!(
            legacy_begin(sp("<!--BEGIN abc.def-->")),
            Ok((
                "",
                Token::LegacyBegin {
                    span: "<!--BEGIN abc.def-->",
                    subject: Expression::Path {
                        span: "abc.def",
                        path: vec![PathPart::Part("abc"), PathPart::Part("def")]
                    }
                }
            ))
        );
        assert_eq_unspan!(
            legacy_begin(sp("<!-- BEGIN call() -->")),
            Ok((
                "",
                Token::LegacyBegin {
                    span: "<!-- BEGIN call() -->",
                    subject: Expression::Helper {
                        span: "call()",
                        name: "call",
                        args: vec![]
                    }
                }
            ))
        );
    }

    #[test]
    fn test_legacy_else() {
        assert_eq_unspan!(
            legacy_else(sp("<!--ELSE-->")),
            Ok((
                "",
                Token::LegacyElse {
                    span: "<!--ELSE-->"
                }
            ))
        );
        assert_eq_unspan!(
            legacy_else(sp("<!-- ELSE -->")),
            Ok((
                "",
                Token::LegacyElse {
                    span: "<!-- ELSE -->"
                }
            ))
        );
    }

    #[test]
    fn test_legacy_end() {
        assert_eq_unspan!(
            legacy_end(sp("<!--END-->")),
            Ok((
                "",
                Token::LegacyEnd {
                    span: "<!--END-->",
                    subject_raw: ""
                }
            ))
        );
        assert_eq_unspan!(
            legacy_end(sp("<!--END abc.def-->")),
            Ok((
                "",
                Token::LegacyEnd {
                    span: "<!--END abc.def-->",
                    subject_raw: "abc.def"
                }
            ))
        );
        assert_eq_unspan!(
            legacy_end(sp("<!-- END -->")),
            Ok((
                "",
                Token::LegacyEnd {
                    span: "<!-- END -->",
                    subject_raw: ""
                }
            ))
        );
        assert_eq_unspan!(
            legacy_end(sp("<!-- ENDIF call() -->")),
            Ok((
                "",
                Token::LegacyEnd {
                    span: "<!-- ENDIF call() -->",
                    subject_raw: "call()"
                }
            ))
        );
    }

    #[test]
    fn test_tokens() {
        fn span_to_str<'a>(
            res: IResult<Span<'a>, Vec<Token<Span<'a>>>>,
        ) -> IResult<&'a str, Vec<Token<&'a str>>> {
            match res {
                Ok((rest, tok)) => Ok((
                    *rest.fragment(),
                    tok.into_iter().map(|t| t.span_to_str()).collect(),
                )),
                Err(err) => Err(
                    err.map(|nom::error::Error { input, code }| nom::error::Error {
                        input: *input.fragment(),
                        code,
                    }),
                ),
            }
        }

        assert_eq_unspan!(
            tokens(
                sp("before {{{ if abc }}} we do one thing {{{ else }}} we do another {{{ end }}} other stuff")
            ),
            Ok((
                "",
                vec![
                    Token::Text("before "),
                    Token::If {
                        span: "{{{ if abc }}}",
                        subject: Expression::Path { span: "abc", path: vec![PathPart::Part("abc")] }
                    },
                    Token::Text(" we do one thing "),
                    Token::Else { span: "{{{ else }}}" },
                    Token::Text(" we do another "),
                    Token::End { span: "{{{ end }}}" },
                    Token::Text(" other stuff"),
                ]
            ))
        );

        assert_eq_unspan!(
            tokens(sp(
                "{{{ if abc }}} we do one thing {{{ else }}} we do another {{{ end }}} other stuff"
            )),
            Ok((
                "",
                vec![
                    Token::If {
                        span: "{{{ if abc }}}",
                        subject: Expression::Path {
                            span: "abc",
                            path: vec![PathPart::Part("abc")]
                        }
                    },
                    Token::Text(" we do one thing "),
                    Token::Else {
                        span: "{{{ else }}}"
                    },
                    Token::Text(" we do another "),
                    Token::End {
                        span: "{{{ end }}}"
                    },
                    Token::Text(" other stuff"),
                ]
            ))
        );

        assert_eq_unspan!(
            tokens(sp("before {{{ each abc }}} for each thing {{{ end }}}")),
            Ok((
                "",
                vec![
                    Token::Text("before "),
                    Token::Each {
                        span: "{{{ each abc }}}",
                        subject: Expression::Path {
                            span: "abc",
                            path: vec![PathPart::Part("abc")]
                        }
                    },
                    Token::Text(" for each thing "),
                    Token::End {
                        span: "{{{ end }}}"
                    },
                ]
            ))
        );

        assert_eq_unspan!(
            tokens(sp("{{{ each abc }}} for each thing {{{ end }}}")),
            Ok((
                "",
                vec![
                    Token::Each {
                        span: "{{{ each abc }}}",
                        subject: Expression::Path {
                            span: "abc",
                            path: vec![PathPart::Part("abc")]
                        }
                    },
                    Token::Text(" for each thing "),
                    Token::End {
                        span: "{{{ end }}}"
                    },
                ]
            ))
        );

        assert_eq_unspan!(
            tokens(sp("{{{ each /abc }}} for each thing {{{ end }}}")),
            Ok((
                "",
                vec![
                    Token::Text("{{{ each /abc }}} for each thing "),
                    Token::End {
                        span: "{{{ end }}}"
                    },
                ]
            ))
        );

        let program = "before \\{{{ each abc }}} for each thing \\{{{ end }}}";
        let source = sp(program);
        assert_eq_unspan!(
            tokens(source),
            Ok((
                "",
                vec![
                    Token::Text("before "),
                    Token::Text("{{{ each abc }}} for each thing "),
                    Token::Text("{{{ end }}}"),
                ]
            ))
        );
    }
}
