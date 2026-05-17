use crate::{arch, perf::SampleData};

#[derive(Clone, Debug)]
pub struct RegFilter {
    groups: Vec<Vec<Predicate>>,
}

#[derive(Clone, Debug)]
struct Predicate {
    reg: usize,
    mask: Option<u64>,
    op: Operator,
    value: u64,
}

#[derive(Clone, Copy, Debug)]
enum Operator {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

impl RegFilter {
    pub fn parse(input: &str) -> anyhow::Result<Self> {
        let tokens = tokenize(input)?;
        if tokens.is_empty() {
            anyhow::bail!("empty register filter");
        }

        let mut parser = Parser { tokens, pos: 0 };
        let groups = parser.parse_filter()?;
        if parser.pos != parser.tokens.len() {
            anyhow::bail!("unexpected token: {}", parser.tokens[parser.pos]);
        }
        Ok(Self { groups })
    }

    pub fn matches(&self, data: &SampleData) -> bool {
        self.groups.iter().any(|group| {
            group.iter().all(|predicate| {
                let Some(reg) = data.regs.get(predicate.reg).copied() else {
                    return false;
                };
                let lhs = predicate.mask.map_or(reg, |mask| reg & mask);
                match predicate.op {
                    Operator::Eq => lhs == predicate.value,
                    Operator::Ne => lhs != predicate.value,
                    Operator::Gt => lhs > predicate.value,
                    Operator::Ge => lhs >= predicate.value,
                    Operator::Lt => lhs < predicate.value,
                    Operator::Le => lhs <= predicate.value,
                }
            })
        })
    }
}

struct Parser {
    tokens: Vec<String>,
    pos: usize,
}

impl Parser {
    fn parse_filter(&mut self) -> anyhow::Result<Vec<Vec<Predicate>>> {
        let mut groups = vec![self.parse_and_group()?];
        while self.consume_any(&["or", "||"]) {
            groups.push(self.parse_and_group()?);
        }
        Ok(groups)
    }

    fn parse_and_group(&mut self) -> anyhow::Result<Vec<Predicate>> {
        let mut group = vec![self.parse_predicate()?];
        while self.consume_any(&["and", "&&"]) {
            group.push(self.parse_predicate()?);
        }
        Ok(group)
    }

    fn parse_predicate(&mut self) -> anyhow::Result<Predicate> {
        let reg_name = self
            .next()
            .ok_or_else(|| anyhow::anyhow!("expected register name"))?;
        let reg = parse_reg(&reg_name)?;
        let mask = if self.consume("&") {
            Some(parse_value(&self.next().ok_or_else(|| {
                anyhow::anyhow!("expected mask value after '&'")
            })?)?)
        } else {
            None
        };
        let op = parse_operator(
            &self
                .next()
                .ok_or_else(|| anyhow::anyhow!("expected comparison operator"))?,
        )?;
        let value = parse_value(
            &self
                .next()
                .ok_or_else(|| anyhow::anyhow!("expected comparison value"))?,
        )?;

        Ok(Predicate {
            reg,
            mask,
            op,
            value,
        })
    }

    fn consume(&mut self, expected: &str) -> bool {
        if self
            .tokens
            .get(self.pos)
            .is_some_and(|token| token.eq_ignore_ascii_case(expected))
        {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn consume_any(&mut self, expected: &[&str]) -> bool {
        expected.iter().any(|token| self.consume(token))
    }

    fn next(&mut self) -> Option<String> {
        let token = self.tokens.get(self.pos)?.clone();
        self.pos += 1;
        Some(token)
    }
}

fn tokenize(input: &str) -> anyhow::Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }

        if matches!(ch, '=' | '!' | '>' | '<' | '&' | '|') {
            let mut token = ch.to_string();
            if chars.peek() == Some(&'=') && matches!(ch, '=' | '!' | '>' | '<') {
                token.push(chars.next().unwrap());
            } else if chars.peek() == Some(&'&') && ch == '&' {
                token.push(chars.next().unwrap());
            } else if chars.peek() == Some(&'|') && ch == '|' {
                token.push(chars.next().unwrap());
            }
            tokens.push(token);
            continue;
        }

        if ch.is_ascii_alphanumeric() || ch == '_' {
            let mut token = ch.to_string();
            while let Some(next) = chars.peek() {
                if next.is_ascii_alphanumeric() || *next == '_' {
                    token.push(chars.next().unwrap());
                } else {
                    break;
                }
            }
            tokens.push(token);
            continue;
        }

        anyhow::bail!("unexpected character in filter: {}", ch);
    }

    Ok(tokens)
}

fn parse_reg(name: &str) -> anyhow::Result<usize> {
    (0..arch::regs_count())
        .find(|idx| arch::id_to_str(*idx).eq_ignore_ascii_case(name))
        .ok_or_else(|| anyhow::anyhow!("unknown register in filter: {}", name))
}

fn parse_operator(token: &str) -> anyhow::Result<Operator> {
    match token {
        "=" | "==" => Ok(Operator::Eq),
        "!=" => Ok(Operator::Ne),
        ">" => Ok(Operator::Gt),
        ">=" => Ok(Operator::Ge),
        "<" => Ok(Operator::Lt),
        "<=" => Ok(Operator::Le),
        _ => anyhow::bail!("invalid comparison operator: {}", token),
    }
}

fn parse_value(token: &str) -> anyhow::Result<u64> {
    if let Some(hex) = token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))
    {
        Ok(u64::from_str_radix(hex, 16)?)
    } else {
        Ok(token.parse()?)
    }
}
