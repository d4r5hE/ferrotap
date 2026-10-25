import re
p='D:/Projects/Ferrotap/src/ui.rs'
s=open(p,encoding='utf-8').read()
new_list='for f in &[FilterField::Method, FilterField::Host, FilterField::Path, FilterField::Url, FilterField::Status, FilterField::ContentType, FilterField::RequestBody, FilterField::ResponseBody, FilterField::DurationMs, FilterField::ResponseSize, FilterField::ProcessName] {'
s,n1=re.subn(r'for f in &\[FilterField::Method, FilterField::Host.*?\] \{',lambda m:new_list,s,count=1,flags=re.S)
s=s.replace('ConditionOp::Regex]','ConditionOp::RegexMatch]')
s=s.replace('self.saved_filters.add(SavedFilter::new(name, rule.clone()));','self.saved_filters.add(&name, rule.clone());')
s=s.replace('self.saved_filters.filters().iter()','self.saved_filters.filters.iter()')
s=s.replace('self.saved_filters.remove(&id);','let _ = self.saved_filters.remove(&id);')
open(p,'w',encoding='utf-8').write(s)
print('list replaced',n1)
g='D:/Projects/Ferrotap/src/g2.rs'
c=open(g,encoding='utf-8').read()
if 'fn describe' not in c:
    c+='''

impl ConditionOp {
    pub fn label(&self) -> &'static str {
        match self {
            ConditionOp::Equals => "equals",
            ConditionOp::NotEquals => "not equals",
            ConditionOp::Contains => "contains",
            ConditionOp::NotContains => "not contains",
            ConditionOp::StartsWith => "starts with",
            ConditionOp::EndsWith => "ends with",
            ConditionOp::GreaterThan => ">",
            ConditionOp::LessThan => "<",
            ConditionOp::RegexMatch => "matches regex",
            ConditionOp::Exists => "exists",
            ConditionOp::DoesNotExist => "does not exist",
        }
    }
}

impl AdvancedFilterRule {
    /// Rule with a single condition.
    pub fn single(cond: FilterCondition) -> Self {
        Self { name: "Quick Filter".to_string(), combinator: Combinator::All, conditions: vec![cond], enabled: true }
    }

    /// Human-readable summary of the rule.
    pub fn describe(&self) -> String {
        let joiner = match self.combinator { Combinator::All => " AND ", _ => " OR " };
        self.conditions.iter()
            .map(|c| format!("{:?} {} '{}'", c.field, c.op.label(), c.value))
            .collect::<Vec<_>>().join(joiner)
    }
}
'''
    open(g,'w',encoding='utf-8').write(c)
print('ok')
