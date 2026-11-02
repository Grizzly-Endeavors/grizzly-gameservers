# The web console as a static site: Vite builds it, nginx serves it. Built by
# .github/workflows/web.yml from the web/ directory alone, so nothing outside
# web/ can affect (or be affected by) this image.
FROM node:24.22.2-alpine AS build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
RUN npm run build

FROM nginxinc/nginx-unprivileged:1.30.6-alpine
# Rendered into /etc/nginx/conf.d at start (an emptyDir in the pod, since the
# root filesystem is read-only).
RUN rm /etc/nginx/conf.d/default.conf
COPY nginx.conf.template /etc/nginx/templates/default.conf.template
COPY --from=build /app/dist /usr/share/nginx/html
EXPOSE 8080
