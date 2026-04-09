---
allowed-tools: Bash(cargo fmt:*), Bash(cargo clippy:*), Bash(git status:*), Bash(git diff:*), Bash(git add:*), Bash(git commit:*), Bash(git push:*), Bash(git log:*), Bash(gh pr:*)
description: Commit changes atomically, push, and create PR
model: claude-haiku-4-5
---

Commit all changes with atomic commits (one commit per logical change), push to the current branch, and create a Pull Request.

Current state:
- Git status: !`git status`
- Git diff (staged and unstaged): !`git diff HEAD`
- Current branch: !`git branch --show-current`
- Recent commits for style reference: !`git log --oneline -5`

Instructions:
1. **First**, run `cargo fmt` to fix formatting, then `cargo clippy -- -D warnings` to check for linting issues
2. Analyze the changes shown above (including any new changes from check:fix)
3. Group related changes by logical action
4. For each group of related changes:
   - Stage only the relevant files with `git add <files>`
   - Create a commit following the **Conventional Commits** format:

     Format: `type(scope): description`

     Types autorisés:
     - `feat` : nouvelle fonctionnalité
     - `fix` : correction de bug
     - `docs` : documentation uniquement
     - `style` : formatage, pas de changement de logique
     - `refactor` : refactoring sans ajout de fonctionnalité ni correction
     - `perf` : amélioration de performance
     - `test` : ajout ou correction de tests
     - `build` : système de build ou dépendances externes
     - `ci` : configuration CI
     - `chore` : tâches diverses (config, tooling)
     - `revert` : annulation d'un commit précédent

     Règles:
     - Le scope est optionnel mais recommandé (ex: `feat(api): add auth endpoint`)
     - La description commence en minuscule, sans point final
     - Utiliser l'impératif (ex: "add" pas "added")
     - Max 100 caractères pour la première ligne
     - Pour un breaking change, ajouter un point d'exclamation ("!") après le type/scope: `feat(api)!: remove v1 endpoint`


     Note: commitlint valide automatiquement le format via le hook commit-msg.

5. Push to the current branch with `git push -u origin <branch>`
6. Check if a PR already exists for the current branch:
   - Use `gh pr list --head <branch> --json number,url,title --limit 1` to check
   - Parse the JSON output to determine if a PR exists
7. If a PR exists:
   - Display: "PR already exists and has been updated with your changes"
   - Show the PR URL, title, and number
   - Optionally run `gh pr view <number>` to show current PR details
8. If no PR exists:
   - Create a Pull Request using `gh pr create` with:
     - A clear title summarizing the main change (en format conventional commit)
     - A body listing all commits made with their descriptions
   - Display: "Created new PR"
9. Return the PR URL to the user
