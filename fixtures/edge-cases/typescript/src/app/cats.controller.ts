import { Controller, Get } from '@nestjs/common';

@Controller({ path: 'cats' })
export class CatsController {
  @Get(['all', 'list'])
  findAll(): string[] {
    return [];
  }
}
