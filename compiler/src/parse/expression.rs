use crate::parse::{
    path::{
        PathBuf,
        PathPart,
    },
    ws,
    Span,
};
use nom::{
    branch::alt,
    bytes::complete::{
        is_a,
        is_not,
        tag,
        take,
    },
    character::complete::alphanumeric1,
    combinator::{
        consumed,
        map,
        opt,
        recognize,
    },
    multi::{
        many0,
        many0_count,
        many1_count,
        separated_list0,
        separated_list1,
    },
    sequence::{
        delimited,
        pair,
        preceded,
    },
    IResult,
    Slice,
};

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum Expression<S> {
    // "this \"works\" as you'd expect"
    StringLiteral(S),
    // a.b.c.d
    Path {
        span: S,
        path: PathBuf<S>,
    },
    // !expr
    Negative {
        span: S,
        expr: Box<Expression<S>>,
    },
    // name(arg0, arg1, arg2, ...)
    Helper {
        span: S,
        name: S,
        args: Vec<Expression<S>>,
    },
    // function.name, arg0, arg1, arg2, ...
    LegacyHelper {
        span: S,
        name: S,
        args: Vec<Expression<S>>,
    },
}

impl<'a> Expression<Span<'a>> {
    pub fn span(&self) -> Span<'a> {
        match self {
            Expression::StringLiteral(span)
            | Expression::Path { span, .. }
            | Expression::Negative { span, .. }
            | Expression::Helper { span, .. }
            | Expression::LegacyHelper { span, .. } => *span,
        }
    }

    pub fn path_from_span(span: Span<'a>) -> Self {
        Expression::Path {
            span,
            path: vec![PathPart::Part(span)],
        }
    }
}

fn string_literal(input: Span) -> IResult<Span, Expression<Span>> {
    map(
        recognize(delimited(
            tag("\""),
            many0_count(alt((preceded(tag("\\"), take(1_usize)), is_not("\\\"")))),
            tag("\""),
        )),
        Expression::StringLiteral,
    )(input)
}

fn identifier(input: Span) -> IResult<Span, Span> {
    let (rest, res): (Span, Span) =
        recognize(many1_count(alt((alphanumeric1, is_a("_-:@")))))(input)?;
    // exclude `-->` from being recognized as part of an expression path
    if res.ends_with("--") && rest.starts_with('>') {
        let split = res.len() - 2;
        Ok((input.slice(split..), input.slice(..split)))
    } else {
        Ok((rest, res))
    }
}

fn path(input: Span) -> IResult<Span, Expression<Span>> {
    alt((
        map(
            alt((
                tag("@root"),
                tag("@key"),
                tag("@index"),
                tag("@value"),
                tag("@first"),
                tag("@last"),
            )),
            Expression::path_from_span,
        ),
        map(
            consumed(pair(
                many0(map(alt((tag("./"), tag("../"))), PathPart::Part)),
                separated_list1(tag("."), map(identifier, PathPart::Part)),
            )),
            |(span, (mut first, mut second))| {
                first.append(&mut second);
                Expression::Path { span, path: first }
            },
        ),
    ))(input)
}

fn negative(input: Span) -> IResult<Span, Expression<Span>> {
    map(
        consumed(preceded(ws(tag("!")), expression)),
        |(span, expr)| Expression::Negative {
            span,
            expr: Box::new(expr),
        },
    )(input)
}

fn helper(input: Span) -> IResult<Span, Expression<Span>> {
    map(
        consumed(pair(
            identifier,
            delimited(
                tag("("),
                separated_list0(tag(","), ws(expression)),
                tag(")"),
            ),
        )),
        |(span, (name, args))| Expression::Helper { span, name, args },
    )(input)
}

fn legacy_helper(input: Span) -> IResult<Span, Expression<Span>> {
    map(
        consumed(pair(
            preceded(tag("function."), identifier),
            opt(preceded(
                ws(tag(",")),
                separated_list0(ws(tag(",")), expression),
            )),
        )),
        |(span, (name, args))| Expression::LegacyHelper {
            span,
            name,
            args: args.unwrap_or_else(|| {
                // Handle legacy helpers without args being implicitly passed `@value`
                vec![Expression::Path {
                    span: span.slice(span.len()..),
                    path: vec![PathPart::Part(Span::new_extra("@value", input.extra))],
                }]
            }),
        },
    )(input)
}

pub fn expression(input: Span) -> IResult<Span, Expression<Span>> {
    // This order is important
    alt((negative, legacy_helper, helper, string_literal, path))(input)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::parse::test::{
        assert_eq,
        assert_eq_unspan,
        sp,
    };

    #[test]
    fn test_string_literal() {
        let src = sp(r#""help" "#);
        assert_eq!(
            string_literal(src),
            Ok((src.slice(6..), Expression::StringLiteral(src.slice(..6))))
        );
        let src = sp(r#""he said \"no!\"" "#);
        assert_eq!(
            string_literal(src),
            Ok((src.slice(17..), Expression::StringLiteral(src.slice(..17))))
        );
        let src = sp("\"\\\\ \\ \"");
        assert_eq!(
            string_literal(src),
            Ok((src.slice(7..), Expression::StringLiteral(src.slice(..7))))
        );
    }

    impl<'a> Expression<Span<'a>> {
        pub fn span_to_str(self) -> Expression<&'a str> {
            match self {
                Expression::StringLiteral(span) => Expression::StringLiteral(*span.fragment()),
                Expression::Path { span, path } => Expression::Path {
                    span: *span.fragment(),
                    path: path.into_iter().map(|p| p.span_to_str()).collect(),
                },
                Expression::Negative { span, expr } => Expression::Negative {
                    span: *span.fragment(),
                    expr: Box::new(expr.span_to_str()),
                },
                Expression::Helper { span, name, args } => Expression::Helper {
                    span: *span.fragment(),
                    name: *name.fragment(),
                    args: args.into_iter().map(|a| a.span_to_str()).collect(),
                },
                Expression::LegacyHelper { span, name, args } => Expression::LegacyHelper {
                    span: *span.fragment(),
                    name: *name.fragment(),
                    args: args.into_iter().map(|a| a.span_to_str()).collect(),
                },
            }
        }
    }

    fn span_to_str<'a>(
        res: IResult<Span<'a>, Expression<Span<'a>>>,
    ) -> IResult<&'a str, Expression<&'a str>> {
        match res {
            Ok((rest, expr)) => Ok((*rest.fragment(), expr.span_to_str())),
            Err(err) => Err(
                err.map(|nom::error::Error { input, code }| nom::error::Error {
                    input: *input.fragment(),
                    code,
                }),
            ),
        }
    }

    #[test]
    fn test_path() {
        assert_eq_unspan!(
            path(sp("a.b.c, what")),
            Ok((
                ", what",
                Expression::Path {
                    span: "a.b.c",
                    path: vec![
                        PathPart::Part("a"),
                        PathPart::Part("b"),
                        PathPart::Part("c")
                    ]
                }
            ))
        );

        assert_eq_unspan!(
            path(sp("@value.c")),
            Ok((
                ".c",
                Expression::Path {
                    span: "@value",
                    path: vec![PathPart::Part("@value")]
                }
            ))
        );

        assert_eq_unspan!(
            path(sp("./../abc.def")),
            Ok((
                "",
                Expression::Path {
                    span: "./../abc.def",
                    path: vec![
                        PathPart::Part("./"),
                        PathPart::Part("../"),
                        PathPart::Part("abc"),
                        PathPart::Part("def")
                    ]
                }
            ))
        );
    }

    #[test]
    fn test_negative() {
        assert_eq_unspan!(
            negative(sp("!a ")),
            Ok((
                " ",
                Expression::Negative {
                    span: "!a",
                    expr: Box::new(Expression::Path {
                        span: "a",
                        path: vec![PathPart::Part("a")]
                    })
                }
            ))
        )
    }

    #[test]
    fn test_helper() {
        assert_eq_unspan!(
            helper(sp("foo(bar, a.b , k) ")),
            Ok((
                " ",
                Expression::Helper {
                    span: "foo(bar, a.b , k)",
                    name: "foo",
                    args: vec![
                        Expression::Path {
                            span: "bar",
                            path: vec![PathPart::Part("bar")]
                        },
                        Expression::Path {
                            span: "a.b",
                            path: vec![PathPart::Part("a"), PathPart::Part("b")]
                        },
                        Expression::Path {
                            span: "k",
                            path: vec![PathPart::Part("k")]
                        }
                    ]
                }
            ))
        )
    }

    #[test]
    fn test_legacy_helper() {
        assert_eq_unspan!(
            legacy_helper(sp("function.foo, bar, a.b, k hf s sgfd")),
            Ok((
                " hf s sgfd",
                Expression::LegacyHelper {
                    span: "function.foo, bar, a.b, k",
                    name: "foo",
                    args: vec![
                        Expression::Path {
                            span: "bar",
                            path: vec![PathPart::Part("bar")]
                        },
                        Expression::Path {
                            span: "a.b",
                            path: vec![PathPart::Part("a"), PathPart::Part("b")]
                        },
                        Expression::Path {
                            span: "k",
                            path: vec![PathPart::Part("k")]
                        }
                    ]
                }
            ))
        );

        assert_eq_unspan!(
            legacy_helper(sp("function.foo")),
            Ok((
                "",
                Expression::LegacyHelper {
                    span: "function.foo",
                    name: "foo",
                    args: vec![Expression::Path {
                        span: "",
                        path: vec![PathPart::Part("@value")]
                    }]
                }
            ))
        );
    }

    #[test]
    fn test_expression() {
        assert_eq_unspan!(
            expression(sp("foo(bar, a.b, function.bar, \"boom\")")),
            Ok((
                "",
                Expression::Helper {
                    span: "foo(bar, a.b, function.bar, \"boom\")",
                    name: "foo",
                    args: vec![
                        Expression::Path {
                            span: "bar",
                            path: vec![PathPart::Part("bar")]
                        },
                        Expression::Path {
                            span: "a.b",
                            path: vec![PathPart::Part("a"), PathPart::Part("b")]
                        },
                        Expression::LegacyHelper {
                            span: "function.bar, \"boom\"",
                            name: "bar",
                            args: vec![Expression::StringLiteral("\"boom\"")]
                        }
                    ]
                }
            ))
        );

        assert_eq_unspan!(
            expression(sp("!foo(bar, a.b)")),
            Ok((
                "",
                Expression::Negative {
                    span: "!foo(bar, a.b)",
                    expr: Box::new(Expression::Helper {
                        span: "foo(bar, a.b)",
                        name: "foo",
                        args: vec![
                            Expression::Path {
                                span: "bar",
                                path: vec![PathPart::Part("bar")]
                            },
                            Expression::Path {
                                span: "a.b",
                                path: vec![PathPart::Part("a"), PathPart::Part("b")]
                            },
                        ]
                    })
                }
            ))
        );
    }
}
