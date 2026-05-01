use anyhow::{Result, bail};

use crate::project::Project;

#[derive(Debug, Default)]
pub struct UndoStack {
    undo: Vec<Project>,
    redo: Vec<Project>,
}

impl UndoStack {
    pub fn checkpoint(&mut self, project: &Project) {
        self.undo.push(project.clone());
        self.redo.clear();
    }

    pub fn undo(&mut self, project: &mut Project) -> Result<()> {
        let previous = self
            .undo
            .pop()
            .ok_or_else(|| anyhow::anyhow!("nothing to undo"))?;
        self.redo.push(project.clone());
        *project = previous;
        Ok(())
    }

    pub fn redo(&mut self, project: &mut Project) -> Result<()> {
        let next = self
            .redo
            .pop()
            .ok_or_else(|| anyhow::anyhow!("nothing to redo"))?;
        self.undo.push(project.clone());
        *project = next;
        Ok(())
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

pub fn apply_undoable<F>(project: &mut Project, undo: &mut UndoStack, edit: F) -> Result<()>
where
    F: FnOnce(&mut Project) -> Result<()>,
{
    let before = project.clone();
    edit(project)?;
    if &before != project {
        undo.checkpoint(&before);
    }
    Ok(())
}

pub fn require_change(changed: bool) -> Result<()> {
    if changed {
        Ok(())
    } else {
        bail!("no change was made")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_and_redo_project_state() {
        let mut project = Project::default();
        let mut undo = UndoStack::default();
        apply_undoable(&mut project, &mut undo, |project| {
            project.tempo_bpm = 132.0;
            Ok(())
        })
        .unwrap();
        assert_eq!(project.tempo_bpm, 132.0);
        undo.undo(&mut project).unwrap();
        assert_eq!(project.tempo_bpm, 120.0);
        undo.redo(&mut project).unwrap();
        assert_eq!(project.tempo_bpm, 132.0);
    }
}
