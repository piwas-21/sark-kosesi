use super::*;
#[derive(sqlx::FromRow)]
struct Group {
    owner_id: String,
    created_at: i64,
    expires_at: i64,
    members_count: i32,
    sealed: bool,
}
#[derive(sqlx::FromRow)]
struct Member {
    id: String,
    sku: String,
    revision: i32,
    units: i32,
    owner_id: String,
    created_at: i64,
    expires_at: i64,
    state: String,
    matches: bool,
}
impl Group {
    fn bind(&self, request: &BasketHoldRequest<'_>, count: usize) -> Result<(), HoldWriteError> {
        if !self.sealed
            || self.owner_id != request.owner_id
            || self.created_at != request.created_at
            || self.expires_at != request.expires_at
            || usize::try_from(self.members_count).ok() != Some(count)
        {
            return Err(HoldWriteError::IdempotencyConflict);
        }
        Ok(())
    }
}
impl Member {
    fn decode(
        self,
        request: &BasketHoldRequest<'_>,
    ) -> Result<(BasketHoldLine, HoldState), HoldWriteError> {
        if !self.matches
            || self.owner_id != request.owner_id
            || self.created_at != request.created_at
            || self.expires_at != request.expires_at
        {
            return Err(HoldWriteError::InvalidRequest);
        }
        let state = match self.state.as_str() {
            "active" => HoldState::Active,
            "released" => HoldState::Released,
            "consumed" => HoldState::Consumed,
            "expired" => HoldState::Expired,
            _ => return Err(HoldWriteError::InvalidRequest),
        };
        let line = BasketHoldLine {
            id: self.id,
            sku: self.sku,
            revision: self.revision,
            units: u16::try_from(self.units).map_err(|_| HoldWriteError::InvalidRequest)?,
        };
        if !valid_line(&line) {
            return Err(HoldWriteError::InvalidRequest);
        }
        Ok((line, state))
    }
}
pub(super) async fn replay(
    tx: &mut Transaction<'_, Postgres>,
    clock: &dyn Clock,
    request: &BasketHoldRequest<'_>,
    lines: &[BasketHoldLine],
    started: i64,
) -> Result<BasketHoldReceipt, HoldWriteError> {
    let group:Group=sqlx::query_as("SELECT left(owner_id,33) AS owner_id,created_at,expires_at,members_count,sealed FROM basket_hold_groups WHERE id=$1 FOR SHARE").bind(request.id).fetch_one(&mut **tx).await?;
    group.bind(request, lines.len())?;
    let mut members:Vec<Member>=sqlx::query_as("SELECT left(h.id,33) AS id,left(h.sku,66) AS sku,h.revision,h.units,left(h.owner_id,33) AS owner_id,h.created_at,h.expires_at,left(h.state,10) AS state,(m.sku,m.revision,m.units,m.group_id)=(h.sku,h.revision,h.units,h.basket_group_id) AS matches FROM basket_hold_members m JOIN stock_holds h ON h.id=m.hold_id WHERE m.group_id=$1 ORDER BY h.id LIMIT 21 FOR SHARE OF h").bind(request.id).fetch_all(&mut **tx).await?;
    members.sort_by(|a, b| a.sku.cmp(&b.sku));
    let mut stored = Vec::with_capacity(members.len());
    let mut holds = Vec::with_capacity(members.len());
    for member in members {
        let (line, state) = member.decode(request)?;
        stored.push(line.clone());
        holds.push(HoldReceipt {
            id: line.id,
            state,
            replayed: true,
        });
    }
    stored.sort_by(|a, b| a.sku.cmp(&b.sku));
    if stored != lines {
        return Err(HoldWriteError::IdempotencyConflict);
    }
    if now(clock)? < started {
        return Err(HoldWriteError::ClockUnavailable);
    }
    // Historical lookup grants no new stock; never reactivate a late or terminal member.
    Ok(BasketHoldReceipt {
        id: request.id.into(),
        holds,
        replayed: true,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn group() -> Group {
        Group {
            owner_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            created_at: 1100,
            expires_at: 1200,
            members_count: 2,
            sealed: true,
        }
    }
    fn member() -> Member {
        Member {
            id: "00000000000000000000000000000001".into(),
            sku: "SET-COVERS".into(),
            revision: 1,
            units: 1,
            owner_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            created_at: 1100,
            expires_at: 1200,
            state: "active".into(),
            matches: true,
        }
    }
    #[test]
    fn stored_manifests_and_members_require_exact_complete_identity_and_known_state() {
        let lines = vec![
            BasketHoldLine {
                id: "00000000000000000000000000000001".into(),
                sku: "SET-COVERS".into(),
                revision: 1,
                units: 1,
            },
            BasketHoldLine {
                id: "00000000000000000000000000000002".into(),
                sku: "SET-FILLED".into(),
                revision: 1,
                units: 1,
            },
        ];
        let request = BasketHoldRequest {
            id: "11111111111111111111111111111111",
            owner_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            created_at: 1100,
            expires_at: 1200,
            lines: &lines,
        };
        assert!(group().bind(&request, 2).is_ok());
        let group_changes: [fn(&mut Group); 6] = [
            |g| g.sealed = false,
            |g| g.owner_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            |g| g.created_at = 1099,
            |g| g.expires_at = 1201,
            |g| g.members_count = 1,
            |g| g.members_count = -1,
        ];
        for change in group_changes {
            let mut g = group();
            change(&mut g);
            assert!(matches!(
                g.bind(&request, 2),
                Err(HoldWriteError::IdempotencyConflict)
            ));
        }
        let member_changes: [fn(&mut Member); 13] = [
            |m| m.matches = false,
            |m| m.owner_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            |m| m.created_at = 1099,
            |m| m.expires_at = 1201,
            |m| m.id = "bad".into(),
            |m| m.id = "g".repeat(32),
            |m| m.sku = "DEMO-A".into(),
            |m| m.revision = 0,
            |m| m.units = 0,
            |m| m.units = 1001,
            |m| m.units = -1,
            |m| m.units = 70000,
            |m| m.state = "building".into(),
        ];
        for change in member_changes {
            let mut m = member();
            change(&mut m);
            assert!(matches!(
                m.decode(&request),
                Err(HoldWriteError::InvalidRequest)
            ));
        }
        for (state, expected) in [
            ("active", HoldState::Active),
            ("released", HoldState::Released),
            ("consumed", HoldState::Consumed),
            ("expired", HoldState::Expired),
        ] {
            let mut m = member();
            m.state = state.into();
            let (line, actual) = m.decode(&request).expect("known immutable snapshot");
            assert_eq!(line, lines[0]);
            assert_eq!(actual, expected);
        }
    }
}
